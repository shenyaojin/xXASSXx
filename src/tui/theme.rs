use ratatui::style::{Color, Modifier, Style};
pub(super) fn muted() -> Style {
    Style::default().fg(Color::DarkGray)
}
pub(super) fn accent() -> Style {
    Style::default().fg(Color::Cyan)
}
pub(super) fn heading() -> Style {
    accent().add_modifier(Modifier::BOLD)
}
pub(super) fn selected() -> Style {
    Style::default()
        .fg(Color::Black)
        .bg(Color::Cyan)
        .add_modifier(Modifier::BOLD)
}
pub(super) fn warning() -> Style {
    Style::default().fg(Color::Yellow)
}
pub(super) fn success() -> Style {
    Style::default().fg(Color::Green)
}
pub(super) fn error() -> Style {
    Style::default().fg(Color::Red)
}
pub(super) fn task(state: &str) -> Style {
    match state {
        "completed" => success(),
        "cancelled" | "stopped" => muted(),
        "needs_attention" => error(),
        "waiting"
        | "draft"
        | "awaiting_authorization"
        | "waiting_peer_authorization"
        | "waiting_user" => warning(),
        _ => accent(),
    }
}
