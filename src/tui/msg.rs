/// Every state change in the TUI is routed through one of these variants.
/// See `update.rs` for the handler.
#[derive(Debug, Clone)]
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

    // Input
    InputSubmit,

    // Toast
    ToastExpire,

    // Quit
    Quit,
}
