#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod gpt;
pub mod media;
// 39 WP-39.4（P1-07）：安装目标全域安全判定。
pub mod surface;

pub use gpt::{
    parse_gpt, read_gpt, write_gpt, Gpt, GptError, Guid, ParsedPartition, PartitionSpec,
    GPT_MAX_PRODUCT_PARTITIONS,
};
pub use media::{BlockDevice, BlockReadError, PartitionReader};
