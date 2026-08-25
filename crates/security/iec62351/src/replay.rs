//! WP-53.6 INV-GOOSE：接收侧防重放滑动窗口.
//!
//! 64 帧右沿窗口（同 IPsec AH/ESP 与本仓 mtls record 层模型）：
//! 维护已见最高序号 `highest` 与 `[highest-63, highest]` 的位图。
//! - `seq > highest`：右沿前移，接受
//! - `seq` 落在窗口内且未见：标记后接受（乱序容忍）
//! - 窗口内已见 → [`ReplayVerdict::Duplicate`]（重放）
//! - `seq ≤ highest-64` → [`ReplayVerdict::TooOld`]（滑出左沿）
//!
//! 序号源 = GOOSE/SV 安全帧 IV 的计数器字段（发送侧每次加密自增，
//! 发送侧零改动）。密钥重协商时窗口必须重置（新 SessionKey ⇒ 新窗口）。

/// 单帧序号的防重放裁决.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayVerdict {
    /// 新帧，接受（含乱序但在窗口内）。
    Accept,
    /// 重放：窗口内已见过该序号。
    Duplicate,
    /// 序号过旧：已滑出窗口左沿。
    TooOld,
}

/// 64 帧防重放滑动窗口.
#[derive(Debug)]
pub struct ReplayWindow {
    /// 已见最高序号。
    highest: u64,
    /// 位图：bit i = 序号 `highest - i` 是否已见（i ∈ 0..64）。
    seen: u64,
}

const WINDOW: u64 = 64;

impl ReplayWindow {
    /// 初始窗口。首个被接受的帧确立右沿（不预设起点，容忍任意起始序号）。
    pub const fn new() -> Self {
        Self {
            highest: 0,
            seen: 0,
        }
    }

    /// 提交一个序号，返回裁决；`Accept` 时内部状态已更新。
    pub fn accept(&mut self, seq: u64) -> ReplayVerdict {
        if seq == 0 {
            // 序号 0 预留（IV 计数器自增起点，永不作为有效帧序号）。
            return ReplayVerdict::TooOld;
        }
        if seq > self.highest {
            let shift = seq - self.highest;
            // 右沿前移：旧位图整体左移，越出 64 位的历史自然淘汰。
            self.seen = if shift >= WINDOW {
                1
            } else {
                (self.seen << shift) | 1
            };
            self.highest = seq;
            return ReplayVerdict::Accept;
        }
        let age = self.highest - seq;
        if age >= WINDOW {
            return ReplayVerdict::TooOld;
        }
        let bit = 1u64 << age;
        if self.seen & bit != 0 {
            return ReplayVerdict::Duplicate;
        }
        self.seen |= bit;
        ReplayVerdict::Accept
    }
}

impl Default for ReplayWindow {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{ReplayVerdict, ReplayWindow};

    #[test]
    fn rw1_first_frame_establishes_edge() {
        let mut w = ReplayWindow::new();
        assert_eq!(w.accept(5), ReplayVerdict::Accept);
        assert_eq!(w.accept(6), ReplayVerdict::Accept);
    }

    #[test]
    fn rw2_exact_duplicate_rejected() {
        let mut w = ReplayWindow::new();
        w.accept(10);
        assert_eq!(w.accept(10), ReplayVerdict::Duplicate);
    }

    #[test]
    fn wp536_replayed_trip_signal_rejected() {
        // INV-GOOSE 场景：跳闸帧（seq=12）原样重发 → Duplicate。
        let mut w = ReplayWindow::new();
        for s in [9u64, 12, 13] {
            let _ = w.accept(s);
        }
        assert_eq!(w.accept(12), ReplayVerdict::Duplicate);
    }

    #[test]
    fn wp536_in_window_reorder_accepted() {
        let mut w = ReplayWindow::new();
        for s in [10u64, 11, 12] {
            assert_eq!(w.accept(s), ReplayVerdict::Accept);
        }
        // 乱序补投 10（仍在窗口内且未收过——此处已收过则应 Duplicate；
        // 改用真正未收过的 9 模拟合法乱序迟到帧）。
        assert_eq!(w.accept(9), ReplayVerdict::Accept);
        assert_eq!(w.accept(8), ReplayVerdict::Accept);
    }

    #[test]
    fn wp536_too_old_beyond_window_rejected() {
        let mut w = ReplayWindow::new();
        // 右沿推进到 200（只收偶数——奇数留给窗口内迟到测试）。
        for s in (100..=200u64).step_by(2) {
            assert_eq!(w.accept(s), ReplayVerdict::Accept);
        }
        assert_eq!(w.accept(135), ReplayVerdict::TooOld, "滑出左沿（age≥64）");
        assert_eq!(
            w.accept(198),
            ReplayVerdict::Duplicate,
            "窗口内已见偶数必须判重"
        );
        assert_eq!(
            w.accept(139),
            ReplayVerdict::Accept,
            "窗口内未见的奇数迟到帧接受"
        );
    }

    #[test]
    fn wp536_large_jump_resets_bitmap_without_overflow() {
        let mut w = ReplayWindow::new();
        w.accept(3);
        // 巨幅跳变（shift ≥ 64）不得发生 u64 移位溢出 panic。
        assert_eq!(w.accept(u64::MAX / 2), ReplayVerdict::Accept);
        assert_eq!(w.accept(4), ReplayVerdict::TooOld);
    }

    #[test]
    fn wp536_sequence_zero_reserved() {
        let mut w = ReplayWindow::new();
        assert_eq!(w.accept(0), ReplayVerdict::TooOld);
    }
}
