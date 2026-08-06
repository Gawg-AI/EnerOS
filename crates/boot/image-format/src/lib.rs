#![no_std]
#![forbid(unsafe_code)]

pub mod gpt;
pub mod media;

pub use gpt::{
    parse_gpt, read_gpt, write_gpt, Gpt, GptError, Guid, ParsedPartition, PartitionSpec,
    GPT_MAX_PRODUCT_PARTITIONS,
};
pub use media::{BlockDevice, BlockReadError, PartitionReader};
