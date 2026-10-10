pub mod approval;
pub mod cancellation;
#[cfg(target_os = "linux")]
mod checkpoint_file;
pub mod cli;
mod context;
pub mod deepseek;
pub mod engine;
pub mod event_sink;
pub mod i18n;
mod instructions;
pub mod model;
pub mod permissions;
pub mod policy;
#[cfg(any(target_os = "macos", all(test, target_os = "linux")))]
mod posix_admin;
#[cfg(any(target_os = "macos", all(test, target_os = "linux")))]
mod posix_file;
pub mod resource;
mod restore_file;
pub mod sandbox;
pub mod state;
#[cfg(target_os = "linux")]
mod syscall_sandbox;
mod tool_file;
pub mod tui;
#[cfg(windows)]
mod windows_file;
pub mod workspace;

pub use approval::{ApprovalGate, ApprovalRequest};
pub use cancellation::CancellationToken;
pub use engine::{
    ReplayStep, RunOptions, get_run_details, latest_display_output, latest_model_reply, list_runs,
    prompt_to_task, replay_messages, run_task, run_task_in_session,
    run_task_in_session_with_approval, run_task_in_session_with_mode,
    run_task_in_session_with_updates, run_task_in_session_with_updates_mode,
    run_task_with_approval, run_task_with_control, run_task_with_event_sink, run_task_with_mode,
};
pub use event_sink::EventSink;
pub use model::*;
pub use permissions::{Permission, PermissionRule, ToolPermissions};
pub use sandbox::ExecutionMode;
pub use workspace::{Checkpoint, Workspace, restore_checkpoint};
