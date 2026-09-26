//! 块命令 **body 文法单源**（蓝图 76 卷二 WP-76.2.2）。
//!
//! **单位（先读这行）**：数据面 = **字节偏移**；boot 面 = **扇区 LBA**。
//! 两个单位并存是 wire 史实（73 批A C9 定型），不是设计——新子码禁止再
//! 引入第三单位。字段名保留 wire 史名：数据面 `off`、boot 面 `lba`。
//!
//! 本模块只承载**布局知识**（哪一段在哪几个字节、小端、定长），零 wire
//! 字节移动（收编前后逐字节由 [`crate::block_cmd`] 的 golden 族冻结）。
//! 信封层职责**不在**此处：子码字节 `payload[0]`、`payload_len`、
//! `DriverRequest/DriverResponse` 打包属 [`crate::runtime`]（含其 `wire`
//! 子模块）——即本模块所有函数进出都是 **body**（子码之后的切片）。
//!
//! 错误面刻意窄：只有一个 [`LenErr::Short`]（长度不足 = 布局违例）。
//! **错误 → status 的映射留给调用方**，因为两端惯例不同且各自是其测试族
//! 的既有契约：服务端 → `ST_INVALID`（[`crate::runtime`] 的状态语义），
//! host 替身 → `Err(())`（transport 语义）。把 status 焊进本模块反而会
//! 逼替身改断言（登记不修项，卷二 §10）。同理，**域规则**（`count == 0`、
//! `count > BLK_READ_MAX`、写数据段为空）留在调用方——它们是"能不能做"，
//! 不是"字节在哪"。

/// 读命令单次数据上限（单源仍在 [`crate::runtime`] 信封域，此处 re-export
/// 是为了让"body 有多长"与"数据段上限"在同一处可读）。
pub use crate::block_cmd::{BLK_READ_MAX, BLK_WRITE_MAX};

/// 信封 → **body** 切片（子码 `payload[0]` 之后的请求段）。
///
/// 这是本模块唯一一处伸进信封的读操作：「body 从第 1 字节起」本身就是
/// 布局知识，收编前在服务端 + 4 替身各写一遍（含 `OFF_BYTES` 局部常量
/// 两份）。`payload_len == 0` 退化为空 body（不是越界 panic），随后一律
/// 被各 `parse_*` 的长度门判 [`LenErr::Short`]——与收编前「`payload_len
/// < 13/9/25/26` → 拒绝」四道门逐案等价。
pub fn body_of(req: &crate::runtime::DriverRequest) -> &[u8] {
    let n = (req.payload_len as usize).saturating_sub(1);
    &req.payload[1..1 + n]
}

/// body 长度不足（布局违例）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LenErr {
    /// 切片短于该子码 body 的定长前缀。
    Short,
}

/// 数据面 `BLK_CMD_READ` body = off(8, **字节偏移**) + count(4, 字节)。
pub fn pack_read(off: u64, count: u32) -> [u8; 12] {
    let mut p = [0u8; 12];
    p[..8].copy_from_slice(&off.to_le_bytes());
    p[8..12].copy_from_slice(&count.to_le_bytes());
    p
}

/// 见 [`pack_read`]（同一布局的解析方向）。
pub fn parse_read(p: &[u8]) -> Result<(u64, u32), LenErr> {
    if p.len() < 12 {
        return Err(LenErr::Short);
    }
    let off = u64::from_le_bytes(p[..8].try_into().unwrap());
    let count = u32::from_le_bytes(p[8..12].try_into().unwrap());
    Ok((off, count))
}

/// 数据面 `BLK_CMD_WRITE` body = off(8, 字节偏移) + data(剩余全部)。
///
/// 借用返回（零拷贝——服务端与替身都直接消费切片）。`data` **可以是空**
/// （body 恰 8B）：那是"写零字节"的域判断，由调用方拒绝，不是布局违例。
pub fn parse_write(p: &[u8]) -> Result<(u64, &[u8]), LenErr> {
    if p.len() < 8 {
        return Err(LenErr::Short);
    }
    let off = u64::from_le_bytes(p[..8].try_into().unwrap());
    Ok((off, &p[8..]))
}

/// `BLK_CMD_HEALTH` 应答 body = blocks(8) + block_size(4) + 折码(1) = 13B
/// （形状冻结：不传 bad_blocks/wear/life 连续量，要传须扩 wire 另立卷）。
pub fn pack_health(blocks: u64, block_size: u32, code: u8) -> [u8; 13] {
    let mut p = [0u8; 13];
    p[..8].copy_from_slice(&blocks.to_le_bytes());
    p[8..12].copy_from_slice(&block_size.to_le_bytes());
    p[12] = code;
    p
}

/// 数据面 `BLK_CMD_WRITE` body 前缀 = off(8, **字节偏移**)，数据段紧随其后
/// 直到 body 末尾（长度上限是域规则，见 [`BLK_WRITE_MAX`]）。
///
/// 刻意只打包前缀不打包整帧：数据段可达 452B，构帧侧（agent-sdk `BlockIo`）
/// 已有一块 `BLK_WRITE_CHUNK + 8` 的复用语缓冲，整帧打包会逼它多拷一遍。
/// 「off 占 body 前 8 字节」这条布局知识因此仍只有一处书写。
pub fn pack_write_prefix(off: u64) -> [u8; 8] {
    off.to_le_bytes()
}

/// boot 面 `BLK_BOOT_INSTALL_WRITE` body 前缀 = lba(8, **扇区**)。
/// 与 [`pack_write_prefix`] 同形不同单位（见模块 doc 首行）。
pub fn pack_boot_write_prefix(lba: u64) -> [u8; 8] {
    lba.to_le_bytes()
}

/// 见 [`pack_health`]。
pub fn parse_health(p: &[u8]) -> Result<(u64, u32, u8), LenErr> {
    if p.len() < 13 {
        return Err(LenErr::Short);
    }
    Ok((
        u64::from_le_bytes(p[..8].try_into().unwrap()),
        u32::from_le_bytes(p[8..12].try_into().unwrap()),
        p[12],
    ))
}

/// boot 面 `BLK_BOOT_READ` body = lba(8, **扇区**) + count(4, **字节**)。
///
/// 混单位是本子码的既有事实（扇区定位 + 字节定量）；与 [`pack_read`] 的
/// 字节偏移**不同单位**，两者字节布局同形——金锁因此成对出现。
pub fn pack_boot_read(lba: u64, count_bytes: u32) -> [u8; 12] {
    pack_read(lba, count_bytes)
}

/// 见 [`pack_boot_read`]。
pub fn parse_boot_read(p: &[u8]) -> Result<(u64, u32), LenErr> {
    parse_read(p)
}

/// boot 面 `BLK_BOOT_INSTALL_WRITE` body = lba(8, 扇区) + data(字节)。
pub fn parse_boot_write(p: &[u8]) -> Result<(u64, &[u8]), LenErr> {
    parse_write(p)
}

/// `BLK_BOOT_TRIAL` body = secver(8) + image_id(16) = 24B。
pub fn pack_boot_trial(secver: u64, image_id: [u8; 16]) -> [u8; 24] {
    let mut p = [0u8; 24];
    p[..8].copy_from_slice(&secver.to_le_bytes());
    p[8..24].copy_from_slice(&image_id);
    p
}

/// 见 [`pack_boot_trial`]。
pub fn parse_boot_trial(p: &[u8]) -> Result<(u64, [u8; 16]), LenErr> {
    if p.len() < 24 {
        return Err(LenErr::Short);
    }
    let mut id = [0u8; 16];
    id.copy_from_slice(&p[8..24]);
    Ok((u64::from_le_bytes(p[..8].try_into().unwrap()), id))
}

/// `BLK_BOOT_CONFIRM` body = slot_is_a(1) + secver(8) + image_id(16) = 25B。
pub fn pack_boot_confirm(slot_is_a: bool, secver: u64, image_id: [u8; 16]) -> [u8; 25] {
    let mut p = [0u8; 25];
    p[0] = u8::from(slot_is_a);
    p[1..9].copy_from_slice(&secver.to_le_bytes());
    p[9..25].copy_from_slice(&image_id);
    p
}

/// 见 [`pack_boot_confirm`]。
///
/// `slot_is_a` 判据是 `p[0] == 1`（**不是**「非零即 A」）——2/0xFF 一律解成
/// B 侧，与收编前的手抄判据逐字一致（该判据本身属 wire 语义，改动 = 破坏
/// 性版本，另立卷）。
pub fn parse_boot_confirm(p: &[u8]) -> Result<(bool, u64, [u8; 16]), LenErr> {
    if p.len() < 25 {
        return Err(LenErr::Short);
    }
    let mut id = [0u8; 16];
    id.copy_from_slice(&p[9..25]);
    Ok((
        p[0] == 1,
        u64::from_le_bytes(p[1..9].try_into().unwrap()),
        id,
    ))
}

/// boot 事务成功应答 body 定长（三臂共用；服务端 `payload_len` 与消费侧
/// 长度判据同源于此，收编前是 `8` 字面量各写一遍）。
pub const SEQ_RESP_LEN: usize = 8;

/// boot 事务成功应答 body = seq(8)（TRIAL / CONFIRM / CANCEL 三臂共用，
/// `payload_len = 8`）。拒绝臂是 word 文本，不在此域（`boot_word_reject`）。
pub fn pack_seq_resp(seq: u64) -> [u8; 8] {
    seq.to_le_bytes()
}

/// 见 [`pack_seq_resp`]。
pub fn parse_seq_resp(p: &[u8]) -> Result<u64, LenErr> {
    if p.len() < 8 {
        return Err(LenErr::Short);
    }
    Ok(u64::from_le_bytes(p[..8].try_into().unwrap()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// golden 值与 [`crate::block_cmd`] 的 WP-76.2.1 金锁族**同源**（同一
    /// 次现服务端跑出的 hex）。权威裁判在 block_cmd（它走整条服务端路径）；
    /// 此处对拍的是"单源函数的字节形"，两处的字面若分叉，block_cmd 族先红。
    const OFF: u64 = 0x1122_3344_5566_7788;
    const COUNT: u32 = 0x99AA_DDBB;
    const GOLDEN_READ_BODY: [u8; 12] = [
        0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11, 0xBB, 0xDD, 0xAA, 0x99,
    ];
    const GOLDEN_HEALTH_BODY: [u8; 13] = [
        0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00,
    ];
    const SECVER: u64 = 0x0B0C_0D0E_0F10_1112;
    const ID: [u8; 16] = [
        0xA0, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5, 0xA6, 0xA7, 0xA8, 0xA9, 0xAA, 0xAB, 0xAC, 0xAD, 0xAE,
        0xAF,
    ];
    const GOLDEN_TRIAL_BODY: [u8; 24] = [
        0x12, 0x11, 0x10, 0x0F, 0x0E, 0x0D, 0x0C, 0x0B, 0xA0, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5, 0xA6,
        0xA7, 0xA8, 0xA9, 0xAA, 0xAB, 0xAC, 0xAD, 0xAE, 0xAF,
    ];
    const GOLDEN_CONFIRM_BODY: [u8; 25] = [
        0x01, 0x12, 0x11, 0x10, 0x0F, 0x0E, 0x0D, 0x0C, 0x0B, 0xA0, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5,
        0xA6, 0xA7, 0xA8, 0xA9, 0xAA, 0xAB, 0xAC, 0xAD, 0xAE, 0xAF,
    ];
    const GOLDEN_SEQ2_BODY: [u8; 8] = [0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];

    #[test]
    fn packs_match_frozen_goldens() {
        assert_eq!(pack_read(OFF, COUNT), GOLDEN_READ_BODY, "READ body");
        assert_eq!(pack_boot_read(OFF, COUNT), GOLDEN_READ_BODY);
        assert_eq!(
            pack_health(4096, 1, 0),
            GOLDEN_HEALTH_BODY,
            "HEALTH body（blocks=4096/bs=1/折码 0）"
        );
        assert_eq!(pack_boot_trial(SECVER, ID), GOLDEN_TRIAL_BODY);
        assert_eq!(pack_boot_confirm(true, SECVER, ID), GOLDEN_CONFIRM_BODY);
        assert_eq!(pack_seq_resp(2), GOLDEN_SEQ2_BODY);
    }

    /// 往返 + 单位不混（数据面 off 与 boot 面 lba 字节同形但语义不同，
    /// 两族各自成对——本测只证 pack/parse 互逆与借用零拷贝）。
    #[test]
    fn roundtrip_every_subcode_body() {
        assert_eq!(parse_read(&pack_read(OFF, COUNT)), Ok((OFF, COUNT)));
        assert_eq!(parse_boot_read(&pack_boot_read(3, 4)), Ok((3, 4)));

        let data = [0xE0u8, 0xE1, 0xE2];
        let mut body = [0u8; 11];
        body[..8].copy_from_slice(&OFF.to_le_bytes());
        body[8..].copy_from_slice(&data);
        let (off, got) = parse_write(&body).unwrap();
        assert_eq!((off, got), (OFF, &data[..]), "写数据段须零拷贝借用");
        let (off, got) = parse_boot_write(&body).unwrap();
        assert_eq!((off, got), (OFF, &data[..]));

        assert_eq!(
            parse_health(&pack_health(7, 512, 4)),
            Ok((7, 512, 4)),
            "折码 4 原样往返（本模块不做语义化）"
        );
        assert_eq!(
            parse_boot_trial(&pack_boot_trial(SECVER, ID)),
            Ok((SECVER, ID))
        );
        assert_eq!(
            parse_boot_confirm(&pack_boot_confirm(false, SECVER, ID)),
            Ok((false, SECVER, ID))
        );
        assert_eq!(parse_seq_resp(&pack_seq_resp(u64::MAX)), Ok(u64::MAX));
    }

    /// 长度门**边界**：差一字节即 `Short`（服务端 `payload_len < 13/9/25/26`
    /// 四道门在收编后由这些边界等价承担）。
    #[test]
    fn length_gates_reject_at_exact_boundary() {
        assert_eq!(parse_read(&GOLDEN_READ_BODY[..11]), Err(LenErr::Short));
        assert_eq!(parse_health(&GOLDEN_HEALTH_BODY[..12]), Err(LenErr::Short));
        assert_eq!(
            parse_boot_trial(&GOLDEN_TRIAL_BODY[..23]),
            Err(LenErr::Short)
        );
        assert_eq!(
            parse_boot_confirm(&GOLDEN_CONFIRM_BODY[..24]),
            Err(LenErr::Short)
        );
        assert_eq!(parse_seq_resp(&GOLDEN_SEQ2_BODY[..7]), Err(LenErr::Short));
        assert_eq!(parse_write(&[0u8; 7]), Err(LenErr::Short));
        assert_eq!(parse_boot_write(&[0u8; 7]), Err(LenErr::Short));
        // body 恰 8B = 空数据段：布局合法（"写零字节"由调用方按域规则拒）。
        assert_eq!(
            parse_write(&[0u8; 8]).map(|(o, d)| (o, d.len())),
            Ok((0, 0))
        );
        // 定长 body 之外的尾字节不影响前缀解析（READ 只认前 12B）。
        assert_eq!(parse_read(&GOLDEN_READ_BODY), Ok((OFF, COUNT)));
        let mut long = GOLDEN_TRIAL_BODY.to_vec();
        long.push(0xFF);
        assert_eq!(parse_boot_trial(&long), Ok((SECVER, ID)));
    }

    /// `slot_is_a` 判据冻结：只有 `1` 是 A（2/0xFF 落 B）——手抄时代的
    /// 非对称判据，收编不得顺手"规整"成 `!= 0`。
    #[test]
    fn confirm_slot_byte_is_eq_one_not_nonzero() {
        for (byte, want) in [(0u8, false), (1, true), (2, false), (0xFF, false)] {
            let mut body = GOLDEN_CONFIRM_BODY;
            body[0] = byte;
            assert_eq!(
                parse_boot_confirm(&body).map(|(is_a, ..)| is_a),
                Ok(want),
                "slot 字节 {byte:#04x} → is_a={want}"
            );
        }
    }

    /// 写 body 的「前缀 + 借用数据段」组合形（构帧侧唯一的布局书写点）。
    #[test]
    fn write_prefix_and_borrowed_data_compose() {
        assert_eq!(pack_write_prefix(OFF), GOLDEN_READ_BODY[..8]);
        assert_eq!(pack_boot_write_prefix(OFF), GOLDEN_READ_BODY[..8]);
        let mut body = [0u8; 8 + 3];
        body[..8].copy_from_slice(&pack_write_prefix(OFF));
        body[8..].copy_from_slice(&[1, 2, 3]);
        assert_eq!(parse_write(&body), Ok((OFF, &[1u8, 2, 3][..])));
        let (lba, data) = parse_boot_write(&body).unwrap();
        assert_eq!((lba, data.len()), (OFF, 3), "boot 面同形不同单位");
    }

    #[test]
    fn capacity_bounds_are_single_sourced() {
        // 上限单源 = 信封 PAYLOAD_CAP（blk_wire 不另立第二套数字）。
        assert_eq!(BLK_READ_MAX, crate::runtime::PAYLOAD_CAP);
        assert_eq!(BLK_WRITE_MAX, crate::runtime::PAYLOAD_CAP - 9);
    }
}
