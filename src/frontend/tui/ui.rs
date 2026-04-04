use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

use crate::permissions::RiskLevel;
use super::app::App;

// ─── Colours ────────────────────────────────────────────────────────────────

const USER_COLOR: Color = Color::Green;
const ASSISTANT_COLOR: Color = Color::Cyan;
const TOOL_COLOR: Color = Color::Yellow;
const ERROR_COLOR: Color = Color::Red;
const SYSTEM_COLOR: Color = Color::DarkGray;
const ACCENT_COLOR: Color = Color::Rgb(180, 140, 255); // soft purple
const STATUS_BG: Color = Color::Rgb(30, 30, 46);       // catppuccin-ish bg
const STATUS_FG: Color = Color::Rgb(166, 173, 200);

// ─── Main draw ──────────────────────────────────────────────────────────────

/// Top-level draw function called every frame.
pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();

    // Queued messages + toast get a fixed-height panel (0 when nothing to show)
    let queued_count = app.queued_user_messages.len();
    let has_toast = app.queue_toast.is_some();
    let has_queued_panel = queued_count > 0 || has_toast;
    let queued_height = if has_queued_panel {
        let mut h = 1u16; // separator line
        h += queued_count as u16;
        if has_toast { h += 1; }
        h
    } else {
        0
    };

    let chunks = Layout::vertical([
        Constraint::Min(3),              // message list (fills remaining space)
        Constraint::Length(queued_height), // queued panel (fixed, 0 when empty)
        Constraint::Length(3),            // input box (fixed height)
        Constraint::Length(1),            // status bar (1 line)
    ])
    .split(area);

    draw_messages(frame, app, chunks[0]);
    if has_queued_panel {
        draw_queued_messages(frame, app, chunks[1]);
    }
    draw_input(frame, app, chunks[2]);
    draw_status_bar(frame, app, chunks[3]);

    // Permission dialog overlay
    if app.pending_permission.is_some() {
        draw_permission_dialog(frame, app);
    }
}

// ─── Message list ───────────────────────────────────────────────────────────

fn draw_messages(frame: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line<'_>> = Vec::new();

    // Render all finalised messages
    for msg in &app.messages {
        match &msg.role {
            super::app::MessageRole::User => {
                lines.push(Line::from(vec![
                    Span::styled("  › ", Style::default().fg(USER_COLOR).bold()),
                    Span::styled(&msg.content, Style::default().fg(USER_COLOR)),
                ]));
                lines.push(Line::from("")); // spacing
            }
            super::app::MessageRole::Assistant => {
                // Split content into lines and prefix the first with the marker
                let msg_lines: Vec<&str> = msg.content.lines().collect();
                for (i, line) in msg_lines.iter().enumerate() {
                    if i == 0 {
                        lines.push(Line::from(vec![
                            Span::styled("  ⟡ ", Style::default().fg(ASSISTANT_COLOR)),
                            Span::raw(*line),
                        ]));
                    } else {
                        lines.push(Line::from(vec![
                            Span::raw("    "),
                            Span::raw(*line),
                        ]));
                    }
                }
                lines.push(Line::from("")); // spacing
            }
            super::app::MessageRole::Tool { name: _, is_error } => {
                let style = if *is_error {
                    Style::default().fg(ERROR_COLOR)
                } else {
                    Style::default().fg(TOOL_COLOR)
                };
                // Truncate long tool lines (char-safe)
                let display = if msg.content.chars().count() > 120 {
                    let truncated: String = msg.content.chars().take(117).collect();
                    format!("{}…", truncated)
                } else {
                    msg.content.clone()
                };
                lines.push(Line::from(vec![
                    Span::styled("  ", Style::default()),
                    Span::styled(display, style),
                ]));
            }
            super::app::MessageRole::System => {
                lines.push(Line::from(vec![
                    Span::styled("  ", Style::default()),
                    Span::styled(&msg.content, Style::default().fg(SYSTEM_COLOR)),
                ]));
            }
        }
    }

    // Streaming text (assistant turn in progress)
    if app.is_streaming && !app.streaming_text.is_empty() {
        let st_lines: Vec<&str> = app.streaming_text.lines().collect();
        for (i, line) in st_lines.iter().enumerate() {
            if i == 0 {
                lines.push(Line::from(vec![
                    Span::styled("  ⟡ ", Style::default().fg(ASSISTANT_COLOR)),
                    Span::raw(*line),
                ]));
            } else {
                lines.push(Line::from(vec![
                    Span::raw("    "),
                    Span::raw(*line),
                ]));
            }
        }
        // Blinking cursor at end of stream
        lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled("▋", Style::default().fg(ACCENT_COLOR).add_modifier(Modifier::SLOW_BLINK)),
        ]));
    }

    // Active tools (spinner)
    for tool in &app.active_tools {
        let spinner = app.spinner_char();
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {} ", spinner),
                Style::default().fg(ACCENT_COLOR),
            ),
            Span::styled(&tool.name, Style::default().fg(TOOL_COLOR).bold()),
            Span::styled(
                format!(" {}", tool.input_preview),
                Style::default().fg(SYSTEM_COLOR),
            ),
        ]));
    }

    // Thinking indicator
    if app.in_thinking && !app.streaming_thinking.is_empty() {
        let spinner = app.spinner_char();
        let preview = if app.streaming_thinking.chars().count() > 60 {
            let truncated: String = app.streaming_thinking.chars().take(57).collect();
            format!("{}…", truncated)
        } else {
            app.streaming_thinking.clone()
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {} 💭 ", spinner),
                Style::default().fg(ACCENT_COLOR),
            ),
            Span::styled(preview, Style::default().fg(SYSTEM_COLOR).italic()),
        ]));
    }

    // "Waiting for response" spinner (between sending and first engine event)
    if app.waiting_for_response && !app.is_streaming && app.active_tools.is_empty() {
        let spinner = app.spinner_char();
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {} ", spinner),
                Style::default().fg(ACCENT_COLOR),
            ),
            Span::styled(
                "Waiting for response…",
                Style::default().fg(SYSTEM_COLOR).italic(),
            ),
        ]));
    }



    // Calculate scroll
    let total_lines = lines.len() as u16;
    let visible_height = area.height.saturating_sub(2); // account for block borders
    let max_scroll = total_lines.saturating_sub(visible_height);

    // Clamp scroll_offset. If auto_scroll (sentinel u16::MAX), pin to bottom.
    // We read scroll_offset through an immutable ref — the actual clamped value
    // is used only for this frame.  The sentinel stays in App until the user
    // scrolls manually.
    let effective_scroll = if app.scroll_offset >= max_scroll {
        max_scroll
    } else {
        app.scroll_offset
    };

    let block = Block::default()
        .borders(Borders::NONE);

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((effective_scroll, 0));

    frame.render_widget(paragraph, area);
}

// ─── Queued messages (fixed position, always visible) ───────────────────────

fn draw_queued_messages(frame: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line<'_>> = Vec::new();

    // Thin separator
    lines.push(Line::from(Span::styled(
        "─".repeat(area.width as usize),
        Style::default().fg(SYSTEM_COLOR),
    )));

    // Queued items
    for queued_msg in &app.queued_user_messages {
        let preview = if queued_msg.chars().count() > 80 {
            let truncated: String = queued_msg.chars().take(77).collect();
            format!("{}...", truncated)
        } else {
            queued_msg.clone()
        };
        lines.push(Line::from(vec![
            Span::styled("  ◌ ", Style::default().fg(ACCENT_COLOR)),
            Span::styled(
                preview,
                Style::default().fg(SYSTEM_COLOR).italic(),
            ),
        ]));
    }

    // Toast (transient error/info)
    if let Some((msg, _)) = &app.queue_toast {
        lines.push(Line::from(vec![
            Span::styled("  ❌ ", Style::default().fg(ERROR_COLOR)),
            Span::styled(
                msg.as_str(),
                Style::default().fg(ERROR_COLOR).italic(),
            ),
        ]));
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, area);
}

// ─── Input box ──────────────────────────────────────────────────────────────

fn draw_input(frame: &mut Frame, app: &App, area: Rect) {
    let queued = app.queued_user_messages.len();
    let title = if queued > 0 {
        format!(" Streaming… · {} queued (auto-send on completion) ", queued)
    } else if app.is_busy() {
        " Streaming… (Ctrl+C to cancel) ".to_string()
    } else {
        " Input ".to_string()
    };

    let style = if app.is_busy() {
        Style::default().fg(SYSTEM_COLOR)
    } else {
        Style::default().fg(Color::White)
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT_COLOR))
        .title(Span::styled(title, Style::default().fg(ACCENT_COLOR)));

    let input_text = Paragraph::new(app.input.as_str())
        .style(style)
        .block(block);

    frame.render_widget(input_text, area);

    // Place cursor inside the input box (only when not busy)
    if !app.is_busy() && app.pending_permission.is_none() {
        // Calculate display width of text before cursor (CJK chars are 2 columns wide)
        let display_cols: usize = app.input.chars().take(app.cursor_pos)
            .map(|c| if c.is_ascii() { 1 } else { 2 })
            .sum();
        // +1 for left border
        let x = area.x + display_cols as u16 + 1;
        let y = area.y + 1; // +1 for top border
        if x < area.x + area.width - 1 {
            frame.set_cursor_position((x, y));
        }
    }
}

// ─── Status bar ─────────────────────────────────────────────────────────────

fn draw_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let cost = crate::cost::calculate_cost(
        app.session_in_tokens,
        app.session_out_tokens,
        &app.pricing,
    );

    let model_display = if app.model_name.is_empty() {
        "unknown".to_string()
    } else {
        app.model_name.clone()
    };

    let status_text = format!(
        " {} │ {}↓ / {}↑ tokens │ ${:.4} ",
        model_display,
        app.session_in_tokens,
        app.session_out_tokens,
        cost,
    );

    let queued_count = app.queued_user_messages.len();
    let right_text = if queued_count > 0 {
        format!(" ⟡ streaming · {} queued ", queued_count)
    } else if app.waiting_for_response && !app.is_streaming {
        " ⟡ waiting ".to_string()
    } else if app.is_streaming {
        " ⟡ streaming ".to_string()
    } else {
        " ready ".to_string()
    };

    // Pad the middle to right-align the right text
    let total_width = area.width as usize;
    let left_len = status_text.len();
    let right_len = right_text.len();
    let padding = total_width.saturating_sub(left_len + right_len);

    let line = Line::from(vec![
        Span::styled(&status_text, Style::default().fg(STATUS_FG).bg(STATUS_BG)),
        Span::styled(
            " ".repeat(padding),
            Style::default().bg(STATUS_BG),
        ),
        Span::styled(
            right_text,
            Style::default()
                .fg(if app.is_busy() {
                    ACCENT_COLOR
                } else {
                    Color::Green
                })
                .bg(STATUS_BG),
        ),
    ]);

    let paragraph = Paragraph::new(line);
    frame.render_widget(paragraph, area);
}

// ─── Permission dialog (overlay) ────────────────────────────────────────────

fn draw_permission_dialog(frame: &mut Frame, app: &App) {
    let perm = match &app.pending_permission {
        Some(p) => p,
        None => return,
    };

    // Size: 60% width, capped at 70 cols, 14 rows
    let width = (frame.area().width * 60 / 100).clamp(40, 70);
    let height = 14u16.min(frame.area().height.saturating_sub(4));
    let x = (frame.area().width.saturating_sub(width)) / 2;
    let y = (frame.area().height.saturating_sub(height)) / 2;
    let dialog_area = Rect::new(x, y, width, height);

    // Clear the area behind the dialog
    frame.render_widget(Clear, dialog_area);

    let risk_color = match perm.risk_level {
        RiskLevel::Low => Color::Green,
        RiskLevel::Medium => Color::Yellow,
        RiskLevel::High => Color::Red,
    };
    let risk_label = match perm.risk_level {
        RiskLevel::Low => "Low",
        RiskLevel::Medium => "Medium",
        RiskLevel::High => "High",
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(risk_color))
        .title(Span::styled(
            " ⚠ Permission Request ",
            Style::default().fg(risk_color).bold(),
        ));

    let inner = block.inner(dialog_area);
    frame.render_widget(block, dialog_area);

    let mut lines: Vec<Line<'_>> = Vec::new();
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("  Tool: ", Style::default().bold()),
        Span::styled(&perm.tool, Style::default().fg(TOOL_COLOR).bold()),
    ]));
    lines.push(Line::from(""));

    // Wrap long descriptions
    let desc_lines: Vec<&str> = perm.description.lines().collect();
    for dl in &desc_lines {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::raw(*dl),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("  Risk: ", Style::default().bold()),
        Span::styled(format!("● {}", risk_label), Style::default().fg(risk_color)),
    ]));
    lines.push(Line::from(""));
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("  [y] ", Style::default().fg(Color::Green).bold()),
        Span::raw("Allow  "),
        Span::styled("[n] ", Style::default().fg(Color::Red).bold()),
        Span::raw("Deny  "),
        Span::styled("[a] ", Style::default().fg(Color::Yellow).bold()),
        Span::raw("Always Allow  "),
        Span::styled("[Esc] ", Style::default().fg(SYSTEM_COLOR)),
        Span::raw("Cancel"),
    ]));

    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false });

    frame.render_widget(paragraph, inner);
}
