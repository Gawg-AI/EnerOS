//! SCL 解析器与信息模型存储.
//!
//! 内置 mini XML DOM 解析器（D6：私有 `xml` 模块，零 unsafe、零第三方依赖），
//! 仅解析 SCL 子集：IED / LDevice / LN0 / LN / DOI / DAI / Val。
//! 解析结果以 `Vec` 存储（D3），并建立 `{ld}/{ln_ref}.{do}.{da}` 路径索引（D4/D9）。

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::do_da::{
    CommonDataClass, DaValue, DataAttribute, DataObject, FunctionalConstraint, Quality, Source,
    Validity,
};
use crate::ld::LogicalDevice;
use crate::ln::{LnClass, LogicalNode};
use crate::xml;
use crate::{Iec61850Model, ModelError};

/// SCL 解析器 + 信息模型（路径索引加速 DA 查找）。
pub struct SclParser {
    lds: Vec<LogicalDevice>,
    path_index: BTreeMap<String, (usize, usize, usize, usize)>,
}

impl SclParser {
    /// 构造空模型。
    pub fn new() -> Self {
        Self {
            lds: Vec::new(),
            path_index: BTreeMap::new(),
        }
    }

    /// 为指定 LD 建立路径索引（键格式 `{ld}/{ln_ref}.{do}.{da}`，D9）。
    fn build_path_index(&mut self, ld_idx: usize) {
        let ld = &self.lds[ld_idx];
        for (ln_idx, ln) in ld.lns.iter().enumerate() {
            let ln_ref = ln_ref_of(ln);
            for (do_idx, data_obj) in ln.do_list.iter().enumerate() {
                for (da_idx, da) in data_obj.da_list.iter().enumerate() {
                    let key = format!(
                        "{}/{}.{}.{}",
                        ld.ld_name, ln_ref, data_obj.do_name, da.da_name
                    );
                    self.path_index
                        .insert(key, (ld_idx, ln_idx, do_idx, da_idx));
                }
            }
        }
    }
}

impl Default for SclParser {
    fn default() -> Self {
        Self::new()
    }
}

impl Iec61850Model for SclParser {
    fn load_scl(&mut self, scl_xml: &str) -> Result<(), ModelError> {
        let root = xml::parse_document(scl_xml)?;
        let mut ieds: Vec<&xml::XmlNode> = Vec::new();
        if root.name == "IED" {
            ieds.push(&root);
        }
        xml::collect_descendants(&root, "IED", &mut ieds);
        for ied in ieds {
            let ied_name = ied.attr("name").unwrap_or("IED");
            let mut ldevs = Vec::new();
            xml::collect_descendants(ied, "LDevice", &mut ldevs);
            for ldev in ldevs {
                let ld_inst = ldev.attr("inst").unwrap_or("LD0");
                let ld_name = format!("{}_{}", ied_name, ld_inst);
                // D12：重复 LD name 显式报错，防止路径索引歧义。
                if self.lds.iter().any(|l| l.ld_name == ld_name) {
                    return Err(ModelError::SclParseError(format!(
                        "duplicate LD name: {}",
                        ld_name
                    )));
                }
                let mut logical_device = LogicalDevice {
                    ld_name: ld_name.clone(),
                    ref_name: ld_name,
                    lns: Vec::new(),
                };
                let mut ln_els = Vec::new();
                xml::collect_descendants(ldev, "LN0", &mut ln_els);
                xml::collect_descendants(ldev, "LN", &mut ln_els);
                for ln_el in ln_els {
                    logical_device.lns.push(build_logical_node(ln_el)?);
                }
                self.lds.push(logical_device);
                let ld_idx = self.lds.len() - 1;
                self.build_path_index(ld_idx);
            }
        }
        Ok(())
    }

    fn get_ld(&self, ld_name: &str) -> Option<&LogicalDevice> {
        self.lds.iter().find(|ld| ld.ld_name == ld_name)
    }

    fn get_ln(&self, ld_name: &str, ln_ref: &str) -> Option<&LogicalNode> {
        self.get_ld(ld_name)?
            .lns
            .iter()
            .find(|ln| ln_ref_of(ln) == ln_ref)
    }

    fn get_da(&self, path: &str) -> Option<&DataAttribute> {
        let &(ld_i, ln_i, do_i, da_i) = self.path_index.get(path)?;
        self.lds
            .get(ld_i)?
            .lns
            .get(ln_i)?
            .do_list
            .get(do_i)?
            .da_list
            .get(da_i)
    }

    fn set_da(&mut self, path: &str, value: DaValue) -> Result<(), ModelError> {
        let idx = *self
            .path_index
            .get(path)
            .ok_or_else(|| ModelError::NotFound(format!("DA path not found: {}", path)))?;
        let da = self
            .lds
            .get_mut(idx.0)
            .and_then(|ld| ld.lns.get_mut(idx.1))
            .and_then(|ln| ln.do_list.get_mut(idx.2))
            .and_then(|data_obj| data_obj.da_list.get_mut(idx.3))
            .ok_or_else(|| ModelError::NotFound(format!("DA path not found: {}", path)))?;
        if core::mem::discriminant(&da.value) != core::mem::discriminant(&value) {
            return Err(ModelError::TypeMismatch(format!(
                "{}: existing {:?}, new {:?}",
                path, da.value, value
            )));
        }
        da.value = value;
        Ok(())
    }

    fn list_lds(&self) -> Vec<&str> {
        self.lds.iter().map(|ld| ld.ld_name.as_str()).collect()
    }
}

/// 计算 LN 引用名（D8：LLN0 无实例后缀；否则 `prefix + class + inst`）。
fn ln_ref_of(ln: &LogicalNode) -> String {
    let class_str = ln_class_str(&ln.ln_class);
    if class_str == "LLN0" && ln.ln_inst == 0 && ln.ln_prefix.is_empty() {
        String::from("LLN0")
    } else {
        format!("{}{}{}", ln.ln_prefix, class_str, ln.ln_inst)
    }
}

fn ln_class_str(c: &LnClass) -> &str {
    match c {
        LnClass::XCBR => "XCBR",
        LnClass::MMXU => "MMXU",
        LnClass::PTRC => "PTRC",
        LnClass::CSWI => "CSWI",
        LnClass::GGIO => "GGIO",
        LnClass::Other(s) => s.as_str(),
    }
}

fn parse_ln_class(s: &str) -> LnClass {
    match s {
        "XCBR" => LnClass::XCBR,
        "MMXU" => LnClass::MMXU,
        "PTRC" => LnClass::PTRC,
        "CSWI" => LnClass::CSWI,
        "GGIO" => LnClass::GGIO,
        other => LnClass::Other(String::from(other)),
    }
}

/// D7：未知 FC 显式报错（不静默默认 ST）。
fn parse_fc(s: &str) -> Result<FunctionalConstraint, ModelError> {
    match s {
        "ST" => Ok(FunctionalConstraint::ST),
        "MX" => Ok(FunctionalConstraint::MX),
        "CO" => Ok(FunctionalConstraint::CO),
        "SP" => Ok(FunctionalConstraint::SP),
        "SG" => Ok(FunctionalConstraint::SG),
        "SE" => Ok(FunctionalConstraint::SE),
        "BR" => Ok(FunctionalConstraint::BR),
        "OR" => Ok(FunctionalConstraint::OR),
        other => Err(ModelError::SclParseError(format!("unknown FC: {}", other))),
    }
}

fn infer_cdc(do_name: &str) -> CommonDataClass {
    match do_name {
        "Pos" | "Beh" => CommonDataClass::DPS,
        "StVal" => CommonDataClass::ENS,
        "W" | "V" | "A" | "Hz" => CommonDataClass::MV,
        "general" => CommonDataClass::SPS,
        other => CommonDataClass::Other(String::from(other)),
    }
}

/// D10：统一值解析规则（无 panic/unwrap）。
fn parse_da_value(val_text: Option<&str>) -> DaValue {
    match val_text {
        Some(raw) => {
            let t = raw.trim();
            if t == "true" || t == "false" {
                DaValue::Bool(t == "true")
            } else if let Ok(i) = t.parse::<i32>() {
                DaValue::Int32(i)
            } else {
                DaValue::StringVal(String::from(t))
            }
        }
        None => DaValue::Bool(false),
    }
}

/// 由 LN/LN0 元素构建逻辑节点（含 DO/DA 子树）。
fn build_logical_node(ln_el: &xml::XmlNode) -> Result<LogicalNode, ModelError> {
    let is_ln0 = ln_el.name == "LN0";
    let default_class = if is_ln0 { "LLN0" } else { "GGIO" };
    let ln_class = parse_ln_class(ln_el.attr("lnClass").unwrap_or(default_class));
    // D8：LN0（LLN0）实例号固定为 0。
    let ln_inst: u16 = if is_ln0 {
        0
    } else {
        ln_el.attr("inst").and_then(|s| s.parse().ok()).unwrap_or(0)
    };
    let ln_prefix = String::from(ln_el.attr("prefix").unwrap_or(""));
    let mut logical_node = LogicalNode {
        ln_class,
        ln_inst,
        ln_prefix,
        do_list: Vec::new(),
    };
    let mut dois = Vec::new();
    xml::collect_descendants(ln_el, "DOI", &mut dois);
    for doi in dois {
        let do_name = String::from(doi.attr("name").unwrap_or(""));
        let cdc = infer_cdc(&do_name);
        let mut data_obj = DataObject {
            do_name,
            da_list: Vec::new(),
            cdc,
        };
        let mut dais = Vec::new();
        xml::collect_descendants(doi, "DAI", &mut dais);
        for dai in dais {
            let da_name = String::from(dai.attr("name").unwrap_or("stVal"));
            let fc = parse_fc(dai.attr("fc").unwrap_or("ST"))?;
            let val_text = xml::find_descendant(dai, "Val").map(|v| v.text.as_str());
            data_obj.da_list.push(DataAttribute {
                da_name,
                fc,
                value: parse_da_value(val_text),
                quality: Quality {
                    validity: Validity::Good,
                    source: Source::Process,
                    test: false,
                    operator_blocked: false,
                },
                timestamp: 0,
            });
        }
        logical_node.do_list.push(data_obj);
    }
    Ok(logical_node)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use alloc::format;
    use alloc::string::String;

    use super::*;
    use crate::xml;

    /// 最小端到端 SCL：1 IED / 1 LD / LLN0 + XCBR1 / Beh + Pos + W。
    const SAMPLE_SCL: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!-- minimal SCL for tests -->
<SCL version="2007" revision="B">
  <IED name="IED1">
    <AccessPoint name="AP1">
      <Server>
        <LDevice inst="LD0">
          <LN0 lnClass="LLN0" lnType="T0">
            <DOI name="Beh">
              <DAI name="stVal" fc="ST"><Val>1</Val></DAI>
            </DOI>
          </LN0>
          <LN lnClass="XCBR" inst="1" prefix="" lnType="T1">
            <DOI name="Pos">
              <DAI name="stVal" fc="ST"><Val>true</Val></DAI>
              <DAI name="q" fc="ST"><Val>0</Val></DAI>
            </DOI>
            <DOI name="W">
              <DAI name="mag" fc="MX"><Val>5000</Val></DAI>
            </DOI>
          </LN>
        </LDevice>
      </Server>
    </AccessPoint>
  </IED>
</SCL>"#;

    fn sample_parser() -> SclParser {
        let mut p = SclParser::new();
        p.load_scl(SAMPLE_SCL).expect("sample SCL parses");
        p
    }

    // ===== SP21：mini XML 解析元素 + 属性 + 文本 =====
    #[test]
    fn test_sp21_mini_xml_element_attr_text() {
        let root = xml::parse_document("<A x=\"1\">hello</A>").expect("parses");
        assert_eq!(root.name, "A");
        assert_eq!(root.attr("x"), Some("1"));
        assert_eq!(root.text, "hello");
        assert!(root.children.is_empty());
    }

    // ===== SP22：mini XML 处理嵌套与自闭合标签 =====
    #[test]
    fn test_sp22_mini_xml_nesting_self_closing() {
        let root = xml::parse_document("<A><B/><C>text</C></A>").expect("parses");
        assert_eq!(root.name, "A");
        assert_eq!(root.children.len(), 2);
        assert_eq!(root.children[0].name, "B");
        assert!(root.children[0].children.is_empty());
        assert_eq!(root.children[1].name, "C");
        assert_eq!(root.children[1].text, "text");
    }

    // ===== SP23：mini XML 跳过声明与注释 =====
    #[test]
    fn test_sp23_mini_xml_skip_decl_comment() {
        let root = xml::parse_document("<?xml version=\"1.0\"?><!-- c --><A/>").expect("parses");
        assert_eq!(root.name, "A");
        // 元素内容中的注释同样跳过
        let root2 = xml::parse_document("<A><!-- inner --><B/></A>").expect("parses");
        assert_eq!(root2.children.len(), 1);
        assert_eq!(root2.children[0].name, "B");
    }

    // ===== SP24：实体转义解码正确性 =====
    #[test]
    fn test_sp24_entity_escape_decoding() {
        let root = xml::parse_document("<A a=\"&quot;x&quot;\">&lt;&amp;&gt;&quot;&apos;</A>")
            .expect("parses");
        assert_eq!(root.text, "<&>\"'");
        assert_eq!(root.attr("a"), Some("\"x\""));
        // CDATA 原文保留、不解码实体
        let cdata = xml::parse_document("<A><![CDATA[&lt;raw>]]></A>").expect("parses");
        assert_eq!(cdata.text, "&lt;raw>");
    }

    // ===== SP25：端到端最小 SCL 解析生成正确 LD/LN/DO/DA 树 =====
    #[test]
    fn test_sp25_end_to_end_minimal_scl() {
        let p = sample_parser();
        let ld = p.get_ld("IED1_LD0").expect("LD exists");
        assert_eq!(ld.ld_name, "IED1_LD0");
        assert_eq!(ld.ref_name, "IED1_LD0");
        assert_eq!(ld.lns.len(), 2);

        let xcbr1 = &ld.lns[1];
        assert_eq!(xcbr1.ln_class, LnClass::XCBR);
        assert_eq!(xcbr1.ln_inst, 1);
        assert_eq!(xcbr1.do_list.len(), 2);

        let pos = &xcbr1.do_list[0];
        assert_eq!(pos.do_name, "Pos");
        assert_eq!(pos.cdc, CommonDataClass::DPS);
        assert_eq!(pos.da_list.len(), 2);
        assert_eq!(pos.da_list[0].da_name, "stVal");
        assert_eq!(pos.da_list[0].fc, FunctionalConstraint::ST);
        assert_eq!(pos.da_list[0].value, DaValue::Bool(true));
        assert_eq!(pos.da_list[0].quality.validity, Validity::Good);
        assert_eq!(pos.da_list[0].timestamp, 0);
        assert_eq!(pos.da_list[1].da_name, "q");
        assert_eq!(pos.da_list[1].value, DaValue::Int32(0));

        let w = &xcbr1.do_list[1];
        assert_eq!(w.do_name, "W");
        assert_eq!(w.cdc, CommonDataClass::MV);
        assert_eq!(w.da_list[0].fc, FunctionalConstraint::MX);
        assert_eq!(w.da_list[0].value, DaValue::Int32(5000));

        // LLN0.Beh.stVal = Int32(1)
        let lln0 = &ld.lns[0];
        assert_eq!(lln0.do_list[0].cdc, CommonDataClass::DPS);
        assert_eq!(lln0.do_list[0].da_list[0].value, DaValue::Int32(1));
    }

    // ===== SP26：LN0（LLN0）解析正确，ln_ref = "LLN0" =====
    #[test]
    fn test_sp26_ln0_lln0_ref() {
        let p = sample_parser();
        let lln0 = p.get_ln("IED1_LD0", "LLN0").expect("LLN0 exists");
        assert_eq!(lln0.ln_inst, 0);
        assert_eq!(lln0.ln_class, LnClass::Other(String::from("LLN0")));
        assert_eq!(lln0.ln_prefix, "");
        // 不带实例后缀的引用名可命中路径索引
        let da = p.get_da("IED1_LD0/LLN0.Beh.stVal").expect("DA exists");
        assert_eq!(da.value, DaValue::Int32(1));
    }

    // ===== SP27：get_da 以 "LD/LN.DO.DA" 路径命中 =====
    #[test]
    fn test_sp27_get_da_by_path() {
        let p = sample_parser();
        let da = p.get_da("IED1_LD0/XCBR1.Pos.stVal").expect("DA exists");
        assert_eq!(da.value, DaValue::Bool(true));
        let mag = p.get_da("IED1_LD0/XCBR1.W.mag").expect("DA exists");
        assert_eq!(mag.value, DaValue::Int32(5000));
        assert!(p.get_da("IED1_LD0/XCBR1.Pos.nope").is_none());
        assert!(p.get_da("NOPE_LD/XCBR1.Pos.stVal").is_none());
    }

    // ===== SP28：get_ld + get_ln + list_lds 返回期望值 =====
    #[test]
    fn test_sp28_get_ld_get_ln_list_lds() {
        let p = sample_parser();
        assert!(p.get_ld("IED1_LD0").is_some());
        assert!(p.get_ld("NOPE").is_none());
        assert!(p.get_ln("IED1_LD0", "XCBR1").is_some());
        assert!(p.get_ln("IED1_LD0", "XCBR2").is_none());
        assert!(p.get_ln("NOPE", "XCBR1").is_none());
        assert_eq!(p.list_lds(), ["IED1_LD0"]);
    }

    // ===== SP29：set_da 成功仅更新 value =====
    #[test]
    fn test_sp29_set_da_success_updates_only_value() {
        let mut p = sample_parser();
        p.set_da("IED1_LD0/XCBR1.Pos.q", DaValue::Int32(42))
            .expect("set ok");
        let da = p.get_da("IED1_LD0/XCBR1.Pos.q").expect("DA exists");
        assert_eq!(da.value, DaValue::Int32(42));
        // 其余字段不变
        assert_eq!(da.da_name, "q");
        assert_eq!(da.fc, FunctionalConstraint::ST);
        assert_eq!(da.quality.validity, Validity::Good);
        assert_eq!(da.quality.source, Source::Process);
        assert_eq!(da.timestamp, 0);
    }

    // ===== SP30：set_da NotFound 返回携带路径的错误 =====
    #[test]
    fn test_sp30_set_da_not_found() {
        let mut p = sample_parser();
        let err = p
            .set_da("IED1_LD0/XCBR1.Pos.nope", DaValue::Int32(1))
            .expect_err("must be NotFound");
        match err {
            ModelError::NotFound(msg) => {
                assert!(msg.contains("IED1_LD0/XCBR1.Pos.nope"));
            }
            other => panic!("expect NotFound, got {:?}", other),
        }
    }

    // ===== SP31：set_da TypeMismatch（判别式不同）=====
    #[test]
    fn test_sp31_set_da_type_mismatch() {
        let mut p = sample_parser();
        let err = p
            .set_da("IED1_LD0/XCBR1.Pos.stVal", DaValue::Int32(1))
            .expect_err("must be TypeMismatch");
        match err {
            ModelError::TypeMismatch(msg) => {
                assert!(msg.contains("IED1_LD0/XCBR1.Pos.stVal"));
            }
            other => panic!("expect TypeMismatch, got {:?}", other),
        }
        // 原值未被修改
        let da = p.get_da("IED1_LD0/XCBR1.Pos.stVal").expect("DA exists");
        assert_eq!(da.value, DaValue::Bool(true));
    }

    // ===== SP32：错误路径 — 畸形 XML（带 line:col）/ 未知 FC / 重复 LD =====
    #[test]
    fn test_sp32_error_paths() {
        // 1) 畸形 XML → SclParseError 携带 line:col
        let mut p = SclParser::new();
        let err = p
            .load_scl("<SCL>\n  <IED name=\"A\">\n</SCL>")
            .expect_err("malformed must fail");
        match err {
            ModelError::SclParseError(msg) => {
                assert!(msg.contains("3:"), "msg should carry line:col: {}", msg);
            }
            other => panic!("expect SclParseError, got {:?}", other),
        }

        // 2) 未知 FC → SclParseError（D7）
        let bad_fc = r#"<SCL><IED name="I"><LDevice inst="L"><LN lnClass="XCBR" inst="1"><DOI name="Pos"><DAI name="stVal" fc="XX"><Val>1</Val></DAI></DOI></LN></LDevice></IED></SCL>"#;
        let err = p.load_scl(bad_fc).expect_err("unknown FC must fail");
        match err {
            ModelError::SclParseError(msg) => {
                assert!(msg.contains("XX"), "msg should name the FC: {}", msg);
            }
            other => panic!("expect SclParseError, got {:?}", other),
        }

        // 3) 重复 LD name → SclParseError（D12，load_scl 可多次调用）
        let mut p2 = sample_parser();
        let err = p2.load_scl(SAMPLE_SCL).expect_err("duplicate LD must fail");
        match err {
            ModelError::SclParseError(msg) => {
                assert!(msg.contains("IED1_LD0"), "msg should name the LD: {}", msg);
            }
            other => panic!("expect SclParseError, got {:?}", other),
        }
    }

    // ===== 性能：1000 DA 查找 < 1ms（D13；蓝图 §6.3/§7.2）=====
    #[test]
    fn test_perf_1000_da_lookups() {
        let mut scl = String::from("<SCL><IED name=\"IED1\"><LDevice inst=\"LD0\">");
        for i in 0..100u16 {
            scl.push_str(&format!("<LN lnClass=\"GGIO\" inst=\"{}\">", i));
            scl.push_str("<DOI name=\"general\">");
            for k in 0..10 {
                scl.push_str(&format!(
                    "<DAI name=\"da{}\" fc=\"ST\"><Val>{}</Val></DAI>",
                    k, k
                ));
            }
            scl.push_str("</DOI></LN>");
        }
        scl.push_str("</LDevice></IED></SCL>");
        let mut p = SclParser::new();
        p.load_scl(&scl).expect("big SCL parses");

        let start = std::time::Instant::now();
        for i in 0..100u16 {
            for k in 0..10 {
                let path = format!("IED1_LD0/GGIO{}.general.da{}", i, k);
                assert!(p.get_da(&path).is_some());
            }
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed.as_millis() < 1,
            "1000 get_da lookups took {:?} (>= 1ms)",
            elapsed
        );
    }
}
