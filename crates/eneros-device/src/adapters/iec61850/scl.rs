//! IEC 61850 SCL (Substation Configuration Language) parser.
//!
//! Parses IEC 61850-6 SCL files (.icd/.cid/.scd) to extract:
//! - IED definitions with logical devices and logical nodes
//! - Data object references (for MMS addressing)
//! - Dataset definitions
//! - Report control block configurations
//! - GOOSE control block configurations
//!
//! # SCL File Types
//!
//! - `.icd`: IED Capability Description (per-IED)
//! - `.scd`: Substation Configuration Description (whole system)
//! - `.cid`: Configured IED Description (per-IED, configured)
//! - `.iid`: Instantiated IED Description
//!
//! # Parsing Approach
//!
//! Uses a minimal XML parser (no external XML dependency). SCL files are
//! large but structurally simple; we extract only the elements needed
//! for runtime addressing (IED/LD/LN/DO/DA/DataSet/RCB/GCB).

use std::collections::HashMap;

/// Parsed SCL document
#[derive(Debug, Clone, Default)]
pub struct SclDocument {
    pub header: SclHeader,
    pub substation: Option<Substation>,
    pub ieds: Vec<Ied>,
    pub data_types: HashMap<String, LNodeType>,
    pub do_types: HashMap<String, DoType>,
    pub da_types: HashMap<String, DaType>,
    pub enums: HashMap<String, EnumType>,
}

#[derive(Debug, Clone, Default)]
pub struct SclHeader {
    pub id: String,
    pub version: String,
    pub revision: String,
    pub tool_id: String,
    pub name_structure: String,
}

#[derive(Debug, Clone, Default)]
pub struct Substation {
    pub name: String,
    pub desc: String,
    pub voltage_levels: Vec<VoltageLevel>,
}

#[derive(Debug, Clone, Default)]
pub struct VoltageLevel {
    pub name: String,
    pub nom_freq: f64,
    pub num_phases: u32,
    pub voltage: f64,
    pub bays: Vec<Bay>,
}

#[derive(Debug, Clone, Default)]
pub struct Bay {
    pub name: String,
    pub desc: String,
    pub conducting_equipment: Vec<ConductingEquipment>,
}

#[derive(Debug, Clone, Default)]
pub struct ConductingEquipment {
    pub name: String,
    pub typ: String,
    pub desc: String,
}

/// IED definition
#[derive(Debug, Clone, Default)]
pub struct Ied {
    pub name: String,
    pub desc: String,
    pub type_ref: String,
    pub manufacturer: String,
    pub config_rev: String,
    pub original_scl_rev: String,
    pub original_scl_version: String,
    pub engineering_settings: bool,
    pub logical_devices: Vec<LogicalDevice>,
}

#[derive(Debug, Clone, Default)]
pub struct LogicalDevice {
    pub name: String,
    pub desc: String,
    pub ld_inst: String,
    pub logical_nodes: Vec<LogicalNode>,
    pub datasets: Vec<DataSet>,
    pub rcbs: Vec<RcbDef>,
    pub gocbs: Vec<GoCbDef>,
}

#[derive(Debug, Clone, Default)]
pub struct LogicalNode {
    pub name: String,
    pub ln_class: String,
    pub ln_inst: String,
    pub prefix: String,
    pub ln_type: String,
}

#[derive(Debug, Clone, Default)]
pub struct DataSet {
    pub name: String,
    pub members: Vec<DatasetMember>,
}

#[derive(Debug, Clone, Default)]
pub struct DatasetMember {
    /// LD name
    pub ld: String,
    /// LN name (with prefix/inst)
    pub ln: String,
    /// DO name (with DA path)
    pub do_da: String,
}

#[derive(Debug, Clone, Default)]
pub struct RcbDef {
    pub name: String,
    pub rcb_type: RcbType,
    pub dataset_ref: String,
    pub report_id: String,
    pub trg_op: String,
    pub intg_pd: u32,
    pub opt_fields: String,
    pub conf_rev: u32,
    pub buffered: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RcbType {
    #[default]
    Unbuffered,
    Buffered,
}

#[derive(Debug, Clone, Default)]
pub struct GoCbDef {
    pub name: String,
    pub dataset_ref: String,
    pub app_id: u32,
    pub dat_set: String,
    pub conf_rev: u32,
    pub fixed_offs: bool,
    pub min_time: u32,
    pub max_time: u32,
}

#[derive(Debug, Clone, Default)]
pub struct LNodeType {
    pub id: String,
    pub ln_class: String,
    pub dos: Vec<DoRef>,
}

#[derive(Debug, Clone, Default)]
pub struct DoRef {
    pub name: String,
    pub type_ref: String,
    pub access_control: String,
}

#[derive(Debug, Clone, Default)]
pub struct DoType {
    pub id: String,
    pub cdc: String, // Common Data Class
    pub das: Vec<DaRef>,
}

#[derive(Debug, Clone, Default)]
pub struct DaRef {
    pub name: String,
    pub fc: String, // Functional Constraint
    pub btype: String,
    pub type_ref: Option<String>,
    pub val_kind: String,
}

#[derive(Debug, Clone, Default)]
pub struct DaType {
    pub id: String,
    pub bdas: Vec<BdaRef>,
}

#[derive(Debug, Clone, Default)]
pub struct BdaRef {
    pub name: String,
    pub btype: String,
    pub type_ref: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct EnumType {
    pub id: String,
    pub values: Vec<(i32, String)>,
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

/// Minimal XML tag parser — yields (tag_name, attributes_str, is_self_closing)
fn parse_tag(s: &str) -> Option<(String, String, bool)> {
    let s = s.trim();
    if !s.starts_with('<') || s.starts_with("</") {
        return None;
    }
    let s = &s[1..];
    let end = s.find('>')?;
    let tag_content = &s[..end];
    let self_closing = tag_content.ends_with('/');
    let tag_content = tag_content.trim_end_matches('/').trim();
    let space = tag_content.find(' ').unwrap_or(tag_content.len());
    let name = tag_content[..space].to_string();
    let attrs = if space < tag_content.len() {
        tag_content[space + 1..].to_string()
    } else {
        String::new()
    };
    Some((name, attrs, self_closing))
}

/// Parse an SCL XML document
pub fn parse_scl(xml: &str) -> Result<SclDocument, String> {
    let mut doc = SclDocument::default();

    // Extract header
    if let Some(h) = extract_element(xml, "Header") {
        doc.header.id = extract_attr(&h, "id").unwrap_or_default();
        doc.header.version = extract_attr(&h, "version").unwrap_or_default();
        doc.header.revision = extract_attr(&h, "revision").unwrap_or_default();
        doc.header.tool_id = extract_attr(&h, "toolID").unwrap_or_default();
        doc.header.name_structure = extract_attr(&h, "nameStructure").unwrap_or_default();
    }

    // Extract IEDs
    for ied_tag in extract_all_elements(xml, "IED") {
        let mut ied = Ied::default();
        ied.name = extract_attr(&ied_tag, "name").unwrap_or_default();
        ied.desc = extract_attr(&ied_tag, "desc").unwrap_or_default();
        ied.type_ref = extract_attr(&ied_tag, "type").unwrap_or_default();
        ied.manufacturer = extract_attr(&ied_tag, "manufacturer").unwrap_or_default();
        ied.config_rev = extract_attr(&ied_tag, "configRev").unwrap_or_default();
        ied.original_scl_rev = extract_attr(&ied_tag, "originalSclRevision").unwrap_or_default();
        ied.original_scl_version = extract_attr(&ied_tag, "originalSclVersion").unwrap_or_default();
        ied.engineering_settings = extract_attr(&ied_tag, "engRight")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

        // Extract LDevice elements within this IED
        for ld_tag in extract_all_elements(&ied_tag, "LDevice") {
            let mut ld = LogicalDevice::default();
            ld.ld_inst = extract_attr(&ld_tag, "inst").unwrap_or_default();
            ld.desc = extract_attr(&ld_tag, "desc").unwrap_or_default();
            ld.name = ld.ld_inst.clone();

            // Extract LN elements
            for ln_tag in extract_all_elements(&ld_tag, "LN") {
                let mut ln = LogicalNode::default();
                ln.ln_class = extract_attr(&ln_tag, "lnClass").unwrap_or_default();
                ln.ln_inst = extract_attr(&ln_tag, "inst").unwrap_or_default();
                ln.prefix = extract_attr(&ln_tag, "prefix").unwrap_or_default();
                ln.ln_type = extract_attr(&ln_tag, "lnType").unwrap_or_default();
                ln.name = format!("{}{}{}", ln.prefix, ln.ln_class, ln.ln_inst);
                ld.logical_nodes.push(ln);
            }

            // Extract LN0 (logical node zero — holds datasets/RCBs)
            for ln0_tag in extract_all_elements(&ld_tag, "LN0") {
                // Datasets
                for ds_tag in extract_all_elements(&ln0_tag, "DataSet") {
                    let mut ds = DataSet::default();
                    ds.name = extract_attr(&ds_tag, "name").unwrap_or_default();
                    for member_tag in extract_all_elements(&ds_tag, "FCDA") {
                        let member = DatasetMember {
                            ld: extract_attr(&member_tag, "ldInst").unwrap_or_default(),
                            ln: extract_attr(&member_tag, "lnClass").unwrap_or_default(),
                            do_da: format!(
                                "{}.{}",
                                extract_attr(&member_tag, "doName").unwrap_or_default(),
                                extract_attr(&member_tag, "daName").unwrap_or_default()
                            ),
                        };
                        ds.members.push(member);
                    }
                    ld.datasets.push(ds);
                }

                // Report control blocks
                for rcb_tag in extract_all_elements(&ln0_tag, "ReportControl") {
                    let mut rcb = RcbDef::default();
                    rcb.name = extract_attr(&rcb_tag, "name").unwrap_or_default();
                    rcb.dataset_ref = extract_attr(&rcb_tag, "datSet").unwrap_or_default();
                    rcb.report_id = extract_attr(&rcb_tag, "rptID").unwrap_or_default();
                    rcb.trg_op = extract_attr(&rcb_tag, "trgOps").unwrap_or_default();
                    rcb.intg_pd = extract_attr(&rcb_tag, "intgPd")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0);
                    rcb.opt_fields = extract_attr(&rcb_tag, "optFields").unwrap_or_default();
                    rcb.conf_rev = extract_attr(&rcb_tag, "confRev")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0);
                    rcb.buffered = extract_attr(&rcb_tag, "buffered")
                        .map(|v| v == "true" || v == "1")
                        .unwrap_or(false);
                    rcb.rcb_type = if rcb.buffered { RcbType::Buffered } else { RcbType::Unbuffered };
                    ld.rcbs.push(rcb);
                }

                // GOOSE control blocks
                for gocb_tag in extract_all_elements(&ln0_tag, "GSEControl") {
                    let mut gocb = GoCbDef::default();
                    gocb.name = extract_attr(&gocb_tag, "name").unwrap_or_default();
                    gocb.dataset_ref = extract_attr(&gocb_tag, "datSet").unwrap_or_default();
                    gocb.app_id = extract_attr(&gocb_tag, "appID")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0);
                    gocb.dat_set = gocb.dataset_ref.clone();
                    gocb.conf_rev = extract_attr(&gocb_tag, "confRev")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0);
                    gocb.fixed_offs = extract_attr(&gocb_tag, "fixedOffs")
                        .map(|v| v == "true" || v == "1")
                        .unwrap_or(false);
                    gocb.min_time = extract_attr(&gocb_tag, "minTime")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0);
                    gocb.max_time = extract_attr(&gocb_tag, "maxTime")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0);
                    ld.gocbs.push(gocb);
                }
            }

            ied.logical_devices.push(ld);
        }

        doc.ieds.push(ied);
    }

    // Extract dataType templates
    if let Some(tpl) = extract_element(xml, "DataTypeTemplates") {
        // LNodeType
        for lnt_tag in extract_all_elements(&tpl, "LNodeType") {
            let mut lnt = LNodeType::default();
            lnt.id = extract_attr(&lnt_tag, "id").unwrap_or_default();
            lnt.ln_class = extract_attr(&lnt_tag, "lnClass").unwrap_or_default();
            for do_tag in extract_all_elements(&lnt_tag, "DO") {
                lnt.dos.push(DoRef {
                    name: extract_attr(&do_tag, "name").unwrap_or_default(),
                    type_ref: extract_attr(&do_tag, "type").unwrap_or_default(),
                    access_control: extract_attr(&do_tag, "accessControl").unwrap_or_default(),
                });
            }
            doc.data_types.insert(lnt.id.clone(), lnt);
        }

        // DOType
        for dot_tag in extract_all_elements(&tpl, "DOType") {
            let mut dot = DoType::default();
            dot.id = extract_attr(&dot_tag, "id").unwrap_or_default();
            dot.cdc = extract_attr(&dot_tag, "cdc").unwrap_or_default();
            for da_tag in extract_all_elements(&dot_tag, "DA") {
                dot.das.push(DaRef {
                    name: extract_attr(&da_tag, "name").unwrap_or_default(),
                    fc: extract_attr(&da_tag, "fc").unwrap_or_default(),
                    btype: extract_attr(&da_tag, "bType").unwrap_or_default(),
                    type_ref: extract_attr(&da_tag, "type"),
                    val_kind: extract_attr(&da_tag, "valKind").unwrap_or_default(),
                });
            }
            doc.do_types.insert(dot.id.clone(), dot);
        }

        // DAType
        for dat_tag in extract_all_elements(&tpl, "DAType") {
            let mut dat = DaType::default();
            dat.id = extract_attr(&dat_tag, "id").unwrap_or_default();
            for bda_tag in extract_all_elements(&dat_tag, "BDA") {
                dat.bdas.push(BdaRef {
                    name: extract_attr(&bda_tag, "name").unwrap_or_default(),
                    btype: extract_attr(&bda_tag, "bType").unwrap_or_default(),
                    type_ref: extract_attr(&bda_tag, "type"),
                });
            }
            doc.da_types.insert(dat.id.clone(), dat);
        }

        // EnumType
        for enum_tag in extract_all_elements(&tpl, "EnumType") {
            let mut et = EnumType::default();
            et.id = extract_attr(&enum_tag, "id").unwrap_or_default();
            for val_tag in extract_all_elements(&enum_tag, "EnumVal") {
                if let Some(ord) = extract_attr(&val_tag, "ord").and_then(|v| v.parse().ok()) {
                    et.values.push((ord, val_tag.split('"').nth(1).unwrap_or("").to_string()));
                }
            }
            doc.enums.insert(et.id.clone(), et);
        }
    }

    Ok(doc)
}

/// Extract the first occurrence of an element (including its content).
/// Handles both paired tags (`<tag>...</tag>`) and self-closing tags (`<tag .../>`).
fn extract_element(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{}", tag);
    let start = xml.find(&open)?;
    // Check it's a real tag boundary
    let after_tag_name = start + 1 + tag.len();
    if after_tag_name >= xml.len() {
        return None;
    }
    let next_char = xml.as_bytes()[after_tag_name];
    if next_char != b' ' && next_char != b'>' && next_char != b'/' && next_char != b'\t' && next_char != b'\n' {
        return None;
    }
    // Find end of opening tag
    let tag_end = xml[start..].find('>')?;
    let abs_tag_end = start + tag_end + 1;
    // Check if self-closing
    if xml[start..abs_tag_end].ends_with("/>") {
        return Some(xml[start..abs_tag_end].to_string());
    }
    // Paired tag
    let close_tag = format!("</{}>", tag);
    let end = xml[abs_tag_end..].find(&close_tag)?;
    Some(xml[start..abs_tag_end + end + close_tag.len()].to_string())
}

/// Extract all occurrences of an element (including content)
fn extract_all_elements(xml: &str, tag: &str) -> Vec<String> {
    let mut results = Vec::new();
    let open = format!("<{}", tag);
    let close_tag = format!("</{}>", tag);
    let mut pos = 0;
    while let Some(start) = xml[pos..].find(&open) {
        let abs_start = pos + start;
        // Check it's a real tag boundary (next char is space, >, or /)
        let after_tag_name = abs_start + 1 + tag.len();
        if after_tag_name >= xml.len() {
            break;
        }
        let next_char = xml.as_bytes()[after_tag_name];
        if next_char != b' ' && next_char != b'>' && next_char != b'/' && next_char != b'\t' && next_char != b'\n' {
            pos = abs_start + 1;
            continue;
        }
        if let Some(end) = xml[abs_start..].find(&close_tag) {
            results.push(xml[abs_start..abs_start + end + close_tag.len()].to_string());
            pos = abs_start + end + close_tag.len();
        } else {
            // Self-closing tag
            if let Some(sc) = xml[abs_start..].find("/>") {
                results.push(xml[abs_start..abs_start + sc + 2].to_string());
                pos = abs_start + sc + 2;
            } else {
                break;
            }
        }
    }
    results
}

impl SclDocument {
    /// Build a flat list of all MMS object references in the document
    pub fn all_object_refs(&self) -> Vec<String> {
        let mut refs = Vec::new();
        for ied in &self.ieds {
            for ld in &ied.logical_devices {
                for ln in &ld.logical_nodes {
                    if let Some(lnt) = self.data_types.get(&ln.ln_type) {
                        for do_ref in &lnt.dos {
                            if let Some(dot) = self.do_types.get(&do_ref.type_ref) {
                                for da in &dot.das {
                                    refs.push(format!(
                                        "{}/{}/{}.{}.{}",
                                        ld.ld_inst, ln.name, do_ref.name, da.name, da.fc
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }
        refs
    }

    /// Find an IED by name
    pub fn find_ied(&self, name: &str) -> Option<&Ied> {
        self.ieds.iter().find(|i| i.name == name)
    }

    /// Count total logical nodes across all IEDs
    pub fn total_logical_nodes(&self) -> usize {
        self.ieds.iter()
            .flat_map(|i| &i.logical_devices)
            .map(|ld| ld.logical_nodes.len())
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_SCL: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<SCL version="2007" revision="B" release="4">
  <Header id="TestSCL" version="1.0" revision="A" toolID="TestTool"/>
  <IED name="IED1" desc="Test IED" type="TestType" manufacturer="TestMfg" configRev="1">
    <AccessPoint name="AP1">
      <Server>
        <LDevice inst="LD0" desc="Logical Device 0">
          <LN0 lnClass="LLN0" inst="">
            <DataSet name="dsGeneric">
              <FCDA ldInst="LD0" lnClass="GGIO1" doName="AnIn1" daName="mag.f" fc="MX"/>
              <FCDA ldInst="LD0" lnClass="GGIO1" doName="Ind1" daName="stVal" fc="ST"/>
            </DataSet>
            <ReportControl name="brcbGeneric01" buffered="true" datSet="dsGeneric"
              rptID="brcbGeneric01" trgOps="dchg qchg gi" intgPd="0"
              optFields="seqNum reasonForInclusion" confRev="1"/>
            <GSEControl name="gocbGeneric01" datSet="dsGeneric" appID="1"
              confRev="1" fixedOffs="false" minTime="10" maxTime="1000"/>
          </LN0>
          <LN lnClass="GGIO1" inst="1" prefix="" lnType="GGIO1_t">
            <DataSet name="dsGGIO1"/>
          </LN>
        </LDevice>
      </Server>
    </AccessPoint>
  </IED>
  <DataTypeTemplates>
    <LNodeType id="GGIO1_t" lnClass="GGIO1">
      <DO name="AnIn1" type="AnIn1_t"/>
      <DO name="Ind1" type="SPS_t"/>
    </LNodeType>
    <DOType id="AnIn1_t" cdc="MV">
      <DA name="mag" bType="Struct" type="Vector_t" fc="MX"/>
      <DA name="q" bType="Quality" fc="ST"/>
    </DOType>
    <DOType id="SPS_t" cdc="SPS">
      <DA name="stVal" bType="BOOLEAN" fc="ST"/>
      <DA name="q" bType="Quality" fc="ST"/>
    </DOType>
    <DAType id="Vector_t">
      <BDA name="f" bType="FLOAT32"/>
      <BDA name="i" bType="INT32"/>
    </DAType>
    <EnumType id="BehKind">
      <EnumVal ord="1">on</EnumVal>
      <EnumVal ord="2">blocked</EnumVal>
    </EnumType>
  </DataTypeTemplates>
</SCL>"#;

    #[test]
    fn test_parse_scl_header() {
        let doc = parse_scl(SAMPLE_SCL).unwrap();
        assert_eq!(doc.header.id, "TestSCL");
        assert_eq!(doc.header.version, "1.0");
        assert_eq!(doc.header.tool_id, "TestTool");
    }

    #[test]
    fn test_parse_scl_ied() {
        let doc = parse_scl(SAMPLE_SCL).unwrap();
        assert_eq!(doc.ieds.len(), 1);
        let ied = &doc.ieds[0];
        assert_eq!(ied.name, "IED1");
        assert_eq!(ied.desc, "Test IED");
        assert_eq!(ied.type_ref, "TestType");
        assert_eq!(ied.manufacturer, "TestMfg");
        assert_eq!(ied.config_rev, "1");
    }

    #[test]
    fn test_parse_scl_logical_device() {
        let doc = parse_scl(SAMPLE_SCL).unwrap();
        let ied = &doc.ieds[0];
        assert_eq!(ied.logical_devices.len(), 1);
        let ld = &ied.logical_devices[0];
        assert_eq!(ld.ld_inst, "LD0");
        assert_eq!(ld.desc, "Logical Device 0");
        // 1 LN0 + 1 LN
        assert_eq!(ld.logical_nodes.len(), 1);
    }

    #[test]
    fn test_parse_scl_dataset() {
        let doc = parse_scl(SAMPLE_SCL).unwrap();
        let ld = &doc.ieds[0].logical_devices[0];
        // dsGeneric is on LN0
        assert_eq!(ld.datasets.len(), 1);
        let ds = &ld.datasets[0];
        assert_eq!(ds.name, "dsGeneric");
        assert_eq!(ds.members.len(), 2);
        assert_eq!(ds.members[0].ld, "LD0");
        assert_eq!(ds.members[0].ln, "GGIO1");
        assert_eq!(ds.members[0].do_da, "AnIn1.mag.f");
    }

    #[test]
    fn test_parse_scl_rcb() {
        let doc = parse_scl(SAMPLE_SCL).unwrap();
        let ld = &doc.ieds[0].logical_devices[0];
        assert_eq!(ld.rcbs.len(), 1);
        let rcb = &ld.rcbs[0];
        assert_eq!(rcb.name, "brcbGeneric01");
        assert_eq!(rcb.rcb_type, RcbType::Buffered);
        assert!(rcb.buffered);
        assert_eq!(rcb.dataset_ref, "dsGeneric");
        assert_eq!(rcb.report_id, "brcbGeneric01");
        assert_eq!(rcb.conf_rev, 1);
    }

    #[test]
    fn test_parse_scl_gocb() {
        let doc = parse_scl(SAMPLE_SCL).unwrap();
        let ld = &doc.ieds[0].logical_devices[0];
        assert_eq!(ld.gocbs.len(), 1);
        let gocb = &ld.gocbs[0];
        assert_eq!(gocb.name, "gocbGeneric01");
        assert_eq!(gocb.app_id, 1);
        assert_eq!(gocb.conf_rev, 1);
        assert!(!gocb.fixed_offs);
        assert_eq!(gocb.min_time, 10);
        assert_eq!(gocb.max_time, 1000);
    }

    #[test]
    fn test_parse_scl_data_types() {
        let doc = parse_scl(SAMPLE_SCL).unwrap();
        assert_eq!(doc.data_types.len(), 1);
        let lnt = doc.data_types.get("GGIO1_t").unwrap();
        assert_eq!(lnt.ln_class, "GGIO1");
        assert_eq!(lnt.dos.len(), 2);
        assert_eq!(lnt.dos[0].name, "AnIn1");
        assert_eq!(lnt.dos[0].type_ref, "AnIn1_t");
    }

    #[test]
    fn test_parse_scl_do_types() {
        let doc = parse_scl(SAMPLE_SCL).unwrap();
        assert_eq!(doc.do_types.len(), 2);
        let dot = doc.do_types.get("AnIn1_t").unwrap();
        assert_eq!(dot.cdc, "MV");
        assert_eq!(dot.das.len(), 2);
        assert_eq!(dot.das[0].name, "mag");
        assert_eq!(dot.das[0].fc, "MX");
    }

    #[test]
    fn test_parse_scl_da_types() {
        let doc = parse_scl(SAMPLE_SCL).unwrap();
        assert_eq!(doc.da_types.len(), 1);
        let dat = doc.da_types.get("Vector_t").unwrap();
        assert_eq!(dat.bdas.len(), 2);
        assert_eq!(dat.bdas[0].name, "f");
        assert_eq!(dat.bdas[0].btype, "FLOAT32");
    }

    #[test]
    fn test_parse_scl_enum_types() {
        let doc = parse_scl(SAMPLE_SCL).unwrap();
        assert_eq!(doc.enums.len(), 1);
        let et = doc.enums.get("BehKind").unwrap();
        assert_eq!(et.values.len(), 2);
        assert_eq!(et.values[0].0, 1);
    }

    #[test]
    fn test_all_object_refs() {
        let doc = parse_scl(SAMPLE_SCL).unwrap();
        let refs = doc.all_object_refs();
        // GGIO1_t has 2 DOs: AnIn1 (2 DAs) + Ind1 (2 DAs) = 4 refs
        assert_eq!(refs.len(), 4);
        assert!(refs.iter().any(|r| r.contains("AnIn1.mag.MX")));
        assert!(refs.iter().any(|r| r.contains("Ind1.stVal.ST")));
    }

    #[test]
    fn test_find_ied() {
        let doc = parse_scl(SAMPLE_SCL).unwrap();
        assert!(doc.find_ied("IED1").is_some());
        assert!(doc.find_ied("nonexistent").is_none());
    }

    #[test]
    fn test_total_logical_nodes() {
        let doc = parse_scl(SAMPLE_SCL).unwrap();
        // 1 LN (LN0 is not counted as a regular LN)
        assert_eq!(doc.total_logical_nodes(), 1);
    }

    #[test]
    fn test_parse_empty_scl() {
        let doc = parse_scl("<SCL></SCL>").unwrap();
        assert!(doc.ieds.is_empty());
    }

    #[test]
    fn test_parse_invalid_xml_returns_empty_doc() {
        // Even with malformed XML, we return an empty doc (graceful degradation)
        let doc = parse_scl("not xml at all").unwrap();
        assert!(doc.ieds.is_empty());
        assert_eq!(doc.header.id, "");
    }
}
