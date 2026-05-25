mod disk;
mod diskpart_cmd;
mod extend;
mod win32_code;
mod winre_inspect;
mod gpt_disk;
mod layout;
mod reagentc;
mod relocation;
mod volume;
mod workflow;

pub use extend::{boot_partition_sectors, extendable_sectors_after_boot};
pub use winre_inspect::{inspect_winre_partition, verify_winre_partition};
pub use workflow::{
    confirm_extend, confirm_relocate, extend_boot_partition, inspect_system_disk, query_winre,
    relocate_workflow,
};
