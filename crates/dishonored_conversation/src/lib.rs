//! Dishonored's conversation system: the original `DisDialogOneShot` / `DisDialogTree` /
//! `DisConversation` / `DisConv_*` graphs read from the cooked packages at runtime
//! ([`load`], feature `load`), their VO resolved in the per-voice `Bk_<GUID>.INT` Wwise
//! banks ([`vo`]), and a deterministic runner ([`runner`]) that plays them on the
//! authoritative simulation (lines timed by VO length, player choices from input, story
//! flags shared with the mission, Kismet outputs as events).
//!
//! Nothing read from the install is stored in the repository: subtitles, choices and VO
//! stay in the player's own game files.

pub mod hooks;
pub mod model;
pub mod runner;

#[cfg(feature = "load")]
pub mod load;
#[cfg(feature = "load")]
pub mod vo;

pub use model::{ConversationDb, NodeId, NodeKind, Target};
pub use runner::{ConvEnv, ConvEvent, EndReason, HookFacts, HookRequest, HookStage, Runner, StartSpec};
