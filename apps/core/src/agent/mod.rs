mod chat;
mod common;
mod health;
pub(crate) mod jobs;
mod memory;
mod presence;
mod tools;

pub use chat::{handle_text, resolve_confirm};
pub(crate) use common::{ask_device, confirm, finish, open_store, say};
pub use common::vault_path;
pub use health::health_tick;
pub(crate) use memory::recall;
pub use presence::{ingest_clipboard, ingest_notice, update_presence};
pub use tools::{glance_saved, save_photo, tick_schedules, tool_result};
pub(crate) use tools::{execute, weather_line};
pub(crate) use jobs::{begin as begin_job, close as close_job};
