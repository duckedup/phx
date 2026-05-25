/// Every state change in the TUI is routed through one of these variants.
/// See `update.rs` for the handler.
pub enum Msg {
    // Scroll
    ScrollUp(usize),
    ScrollDown(usize),
    ScrollToBottom,

    // Tab management
    TabSwitch(usize),
    TabClose(usize),

    // Focus
    PanelFocusSet(bool),

    // Sidebar
    SidebarScrollUp,
    SidebarScrollDown,
    SidebarNavigateUp,
    SidebarNavigateDown,
    SidebarDismissAgent(String),
    SidebarDismissSelected,
    SidebarActivateSelected,
    SidebarSelect(crate::tui::components::sidebar::SidebarSelection),

    // File viewer
    FileViewerScrollUp(usize),
    FileViewerScrollDown(usize),
    FileViewerSwitchToChat,
    FileViewerSwitchTab(usize),
    FileViewerCloseTab(usize),
    FileViewerOpenFile(std::path::PathBuf),

    // Picker
    PickerClose,
    PickerClear,

    // Selection
    SelectionClear,
    HoverLine(Option<usize>),
    FileViewerHoverClose(Option<usize>),

    // Conversation events
    ConvStreamToken {
        tab_idx: usize,
        text: String,
    },
    ConvAssistantMessage {
        tab_idx: usize,
        text: String,
    },
    ConvToolCall {
        tab_idx: usize,
        summary: String,
    },
    ConvToolResult {
        tab_idx: usize,
        output: String,
    },
    ConvContextLoaded {
        tab_idx: usize,
        names: Vec<String>,
    },
    ConvContextCompacted {
        tab_idx: usize,
        removed: usize,
        remaining: usize,
    },
    ConvRetrying {
        tab_idx: usize,
        attempt: u32,
        max_retries: u32,
        wait_secs: u64,
        error: String,
    },
    ConvRetryRecovered {
        tab_idx: usize,
        attempts: u32,
    },
    ConvError {
        tab_idx: usize,
        message: String,
    },
    ConvCancelled {
        tab_idx: usize,
        agent_idx: usize,
    },
    ConvDone {
        tab_idx: usize,
        agent_idx: usize,
    },
    ConvInteractiveUi {
        tool_name: String,
        fields: Vec<crate::shared::ui_field_types::UiField>,
        response_tx: tokio::sync::oneshot::Sender<Option<String>>,
    },

    // Tool detail panel
    ToolDetailOpen {
        tool_name: String,
        content: String,
    },

    // Modals
    ToolFormSubmit {
        answers: String,
        tool_name: String,
        args_json: String,
    },
    ToolFormDismiss,
    ModelsPageDismiss,
    OnboardingDismiss,

    // Input
    InputSubmit,

    // Toast
    ToastExpire,

    // Quit
    Quit,
}
