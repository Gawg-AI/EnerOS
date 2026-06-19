//! Typical connection modes for distribution networks (inspired by cnpower's
//! `topology/connection_modes.py`).
//!
//! Provides standard connection patterns with reliability indices (SAIFI/SAIDI)
//! and topology templates for Chinese 10kV distribution networks.

use serde::{Deserialize, Serialize};

/// Typical connection mode for a distribution feeder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionMode {
    /// 单回辐射（可靠性最低，无备用）
    SingleRadial,
    /// 单联络（两个馈线末端联络）
    SingleTie,
    /// 双联络（三个馈线两两联络）
    DoubleTie,
    /// 三段三联络（高可靠性，城市核心区）
    ThreeSegmentThreeTie,
    /// 多段多联络（最高可靠性）
    MultiSegmentMultiTie,
    /// 单环网（电缆网络常用）
    SingleRing,
    /// 双环网（高可靠性电缆网络）
    DoubleRing,
}

impl ConnectionMode {
    /// 获取接线模式描述
    pub fn description(&self) -> &'static str {
        match self {
            Self::SingleRadial => "单回辐射：单电源供电，无联络，可靠性最低",
            Self::SingleTie => "单联络：两个馈线末端联络，N-1 可通过",
            Self::DoubleTie => "双联络：三个馈线两两联络，N-1 可通过",
            Self::ThreeSegmentThreeTie => "三段三联络：城市核心区高可靠性接线",
            Self::MultiSegmentMultiTie => "多段多联络：最高可靠性，重要负荷区",
            Self::SingleRing => "单环网：电缆网络常用，环网供电",
            Self::DoubleRing => "双环网：高可靠性电缆网络，双环网供电",
        }
    }

    /// SAIFI（系统平均停电频率指数，次/户·年）
    pub fn saifi(&self) -> f64 {
        match self {
            Self::SingleRadial => 2.0,
            Self::SingleTie => 1.0,
            Self::DoubleTie => 0.7,
            Self::ThreeSegmentThreeTie => 0.4,
            Self::MultiSegmentMultiTie => 0.3,
            Self::SingleRing => 0.8,
            Self::DoubleRing => 0.4,
        }
    }

    /// SAIDI（系统平均停电持续时间指数，小时/户·年）
    pub fn saidi(&self) -> f64 {
        match self {
            Self::SingleRadial => 6.0,
            Self::SingleTie => 2.5,
            Self::DoubleTie => 1.5,
            Self::ThreeSegmentThreeTie => 0.8,
            Self::MultiSegmentMultiTie => 0.5,
            Self::SingleRing => 2.0,
            Self::DoubleRing => 1.0,
        }
    }

    /// 供电可靠性 RS-1 (%)
    pub fn reliability_rs1(&self) -> f64 {
        let saidi = self.saidi();
        let total_hours = 8760.0;
        (1.0 - saidi / total_hours) * 100.0
    }

    /// 是否满足 N-1 校验
    pub fn satisfies_n1(&self) -> bool {
        !matches!(self, Self::SingleRadial)
    }

    /// 适用区域类型
    pub fn applicable_area(&self) -> &'static str {
        match self {
            Self::SingleRadial => "D/E类供电区，农村地区",
            Self::SingleTie => "C类供电区，郊区",
            Self::DoubleTie => "B类供电区，一般城区",
            Self::ThreeSegmentThreeTie => "A类供电区，中心城区",
            Self::MultiSegmentMultiTie => "A类供电区，核心负荷区",
            Self::SingleRing => "B/C类供电区，电缆网络",
            Self::DoubleRing => "A类供电区，高可靠性电缆网络",
        }
    }

    /// 拓扑模板参数
    pub fn topology_template(&self) -> TopologyTemplate {
        match self {
            Self::SingleRadial => TopologyTemplate {
                bus_count: 5,
                line_count: 4,
                switch_count: 1,
                tie_switch_count: 0,
                segment_count: 1,
                max_load_mw: 5.0,
            },
            Self::SingleTie => TopologyTemplate {
                bus_count: 10,
                line_count: 9,
                switch_count: 3,
                tie_switch_count: 1,
                segment_count: 2,
                max_load_mw: 10.0,
            },
            Self::DoubleTie => TopologyTemplate {
                bus_count: 15,
                line_count: 14,
                switch_count: 5,
                tie_switch_count: 2,
                segment_count: 3,
                max_load_mw: 15.0,
            },
            Self::ThreeSegmentThreeTie => TopologyTemplate {
                bus_count: 12,
                line_count: 11,
                switch_count: 6,
                tie_switch_count: 3,
                segment_count: 3,
                max_load_mw: 12.0,
            },
            Self::MultiSegmentMultiTie => TopologyTemplate {
                bus_count: 20,
                line_count: 19,
                switch_count: 10,
                tie_switch_count: 4,
                segment_count: 4,
                max_load_mw: 20.0,
            },
            Self::SingleRing => TopologyTemplate {
                bus_count: 8,
                line_count: 8,
                switch_count: 4,
                tie_switch_count: 1,
                segment_count: 2,
                max_load_mw: 8.0,
            },
            Self::DoubleRing => TopologyTemplate {
                bus_count: 16,
                line_count: 16,
                switch_count: 8,
                tie_switch_count: 2,
                segment_count: 4,
                max_load_mw: 16.0,
            },
        }
    }

    /// 根据网络参数自动识别接线模式
    pub fn match_network(
        bus_count: usize,
        tie_switch_count: usize,
        segment_count: usize,
        is_ring: bool,
    ) -> Self {
        if is_ring {
            if tie_switch_count >= 2 {
                return Self::DoubleRing;
            }
            return Self::SingleRing;
        }

        if tie_switch_count == 0 {
            return Self::SingleRadial;
        }

        if segment_count >= 4 && tie_switch_count >= 4 {
            return Self::MultiSegmentMultiTie;
        }

        if segment_count == 3 && tie_switch_count == 3 {
            return Self::ThreeSegmentThreeTie;
        }

        if tie_switch_count >= 2 {
            return Self::DoubleTie;
        }

        if tie_switch_count == 1 {
            return Self::SingleTie;
        }

        // 默认按规模判断
        if bus_count > 15 {
            Self::MultiSegmentMultiTie
        } else if bus_count > 8 {
            Self::DoubleTie
        } else {
            Self::SingleRadial
        }
    }
}

/// 拓扑模板参数
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopologyTemplate {
    /// 母线数量
    pub bus_count: usize,
    /// 线路数量
    pub line_count: usize,
    /// 开关数量
    pub switch_count: usize,
    /// 联络开关数量
    pub tie_switch_count: usize,
    /// 分段数量
    pub segment_count: usize,
    /// 最大负荷 (MW)
    pub max_load_mw: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reliability_indices() {
        let radial = ConnectionMode::SingleRadial;
        assert_eq!(radial.saifi(), 2.0);
        assert_eq!(radial.saidi(), 6.0);
        assert!(!radial.satisfies_n1());

        let multi = ConnectionMode::MultiSegmentMultiTie;
        assert_eq!(multi.saifi(), 0.3);
        assert_eq!(multi.saidi(), 0.5);
        assert!(multi.satisfies_n1());
    }

    #[test]
    fn test_reliability_rs1() {
        let radial = ConnectionMode::SingleRadial;
        let rs1 = radial.reliability_rs1();
        // SAIDI=6h, RS-1 = (1 - 6/8760) * 100 ≈ 99.93%
        assert!(rs1 > 99.9 && rs1 < 100.0);
    }

    #[test]
    fn test_topology_template() {
        let template = ConnectionMode::ThreeSegmentThreeTie.topology_template();
        assert_eq!(template.segment_count, 3);
        assert_eq!(template.tie_switch_count, 3);
        assert_eq!(template.max_load_mw, 12.0);
    }

    #[test]
    fn test_match_network_radial() {
        let mode = ConnectionMode::match_network(5, 0, 1, false);
        assert_eq!(mode, ConnectionMode::SingleRadial);
    }

    #[test]
    fn test_match_network_single_tie() {
        let mode = ConnectionMode::match_network(10, 1, 2, false);
        assert_eq!(mode, ConnectionMode::SingleTie);
    }

    #[test]
    fn test_match_network_ring() {
        let mode = ConnectionMode::match_network(8, 1, 2, true);
        assert_eq!(mode, ConnectionMode::SingleRing);

        let mode = ConnectionMode::match_network(16, 2, 4, true);
        assert_eq!(mode, ConnectionMode::DoubleRing);
    }

    #[test]
    fn test_match_network_three_segment() {
        let mode = ConnectionMode::match_network(12, 3, 3, false);
        assert_eq!(mode, ConnectionMode::ThreeSegmentThreeTie);
    }

    #[test]
    fn test_applicable_area() {
        assert!(ConnectionMode::SingleRadial.applicable_area().contains("农村"));
        assert!(ConnectionMode::ThreeSegmentThreeTie.applicable_area().contains("中心城区"));
    }
}
