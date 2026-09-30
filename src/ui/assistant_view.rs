//! The assistant pane of a window and its conversations with Codex.
use super::*;
use crate::{
    assistant::{
        AccountKind, AssistantEvent, HarnessSnapshot, HistoryTurn, MAX_MESSAGE_BYTES, TitleRequest,
        ToolCall, TurnRequest,
        broker::{
            ActionTarget, ConnectionContext, ConnectionState, MAX_CONTEXT_SQL_BYTES, QueryState,
            ResultSummary, SelectedTabContext, TabSummary, WorkspaceContext, bound_text,
            context_statement_ranges, sql_window,
        },
        history_item_text,
        service::{
            Command as AssistantCommand, Event as AssistantServiceEvent, Operation, Service,
        },
        tools,
    },
    model::{
        AssistantConversation, AssistantExecutionMode, AssistantTitleSource,
        MAX_ASSISTANT_CONVERSATION_TITLE,
    },
};
use gpui_kit::assets::IconName as AssetIconName;
use gpui_kit::base::SelectableText;
use gpui_kit::component::{
    Icon, Selectable,
    alert::Alert,
    bubble::{Bubble, BubbleContent, BubbleVariant},
    button::{ButtonRounded, DropdownButton},
    collapsible::Collapsible,
    empty::{
        Empty, EmptyContent, EmptyDescription, EmptyHeader, EmptyMedia, EmptyMediaVariant,
        EmptyTitle,
    },
    h_flex,
    input::{Textarea, TextareaState},
    menu::DropdownMenu,
    message::{Message, MessageAlignment, MessageContent},
    shimmer::ShimmerText,
    spinner::Spinner,
    text::{TextView, TextViewStyle},
    v_flex,
};
use serde_json::{Value, json};
use std::ops::Range;
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    path::PathBuf,
    rc::Rc,
    time::{SystemTime, UNIX_EPOCH},
};

pub(super) fn unix_now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

mod codex;
mod conversations;
mod events;
mod messages;
mod pane;
mod session;
mod sign_in;
mod thread_list;
mod transcript;
mod transcript_view;

use codex::*;
use conversations::*;
use pane::*;
use session::*;
use transcript::*;

pub use session::CODEX_IDLE_TIMEOUT;
pub(super) use session::{
    AppendedQuery, AssistantPanelState, ComposerTarget, PendingQuery, PendingQueryKind, Status,
    ThreadStatus,
};
pub(super) use transcript::{ToolActivity, ToolKind, ToolState, TranscriptEntry};
