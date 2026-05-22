mod disk;
mod extend;
mod gpt_disk;
mod layout;
mod reagentc;
mod relocation;
mod volume;
mod workflow;

pub use extend::extendable_sectors_after_boot;
pub use workflow::{
    confirm_extend, confirm_relocate, extend_boot_partition, inspect_system_disk, query_winre,
    relocate_workflow,
};
