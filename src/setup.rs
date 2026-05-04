use crossterm::{
    event::{self, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Paragraph},
};

use crate::config::{AsrConfig, WhisperASRConfig};

type Term = Terminal<CrosstermBackend<std::io::Stdout>>;

#[derive(Clone, Copy, PartialEq)]
enum Platform {
    Whisper,
    WebVosk,
}

impl Platform {
    fn label(self) -> &'static str {
        match self {
            Platform::Whisper => "Whisper",
            Platform::WebVosk => "WebVosk",
        }
    }

    fn all() -> [Platform; 2] {
        [Platform::Whisper, Platform::WebVosk]
    }
}

#[derive(Clone, Copy, PartialEq)]
#[allow(clippy::upper_case_acronyms)]
enum Provider {
    OpenAI,
    ByteFuture,
    Groq,
    GLM,
    Custom,
}

impl Provider {
    fn label(self) -> &'static str {
        match self {
            Provider::OpenAI => "OpenAI",
            Provider::ByteFuture => "ByteFuture",
            Provider::Groq => "Groq",
            Provider::GLM => "GLM",
            Provider::Custom => "Custom",
        }
    }

    fn all() -> [Provider; 5] {
        [
            Provider::OpenAI,
            Provider::ByteFuture,
            Provider::Groq,
            Provider::GLM,
            Provider::Custom,
        ]
    }

    fn defaults(self) -> (&'static str, &'static str) {
        // (url, model)
        match self {
            Provider::OpenAI => (
                "https://api.openai.com/v1/audio/transcriptions",
                "whisper-1",
            ),
            Provider::ByteFuture => (
                "https://models.bytefuture.ai/v1/audio/transcriptions",
                "groq/whisper-large-v3",
            ),
            Provider::Groq => (
                "https://api.groq.com/openai/v1/audio/transcriptions",
                "whisper-large-v3-turbo",
            ),
            Provider::GLM => (
                "https://open.bigmodel.cn/api/paas/v4/audio/transcriptions",
                "glm-asr-2512",
            ),
            Provider::Custom => ("", ""),
        }
    }
}

/// Field descriptor for the Whisper form.
struct Field {
    label: &'static str,
    default: &'static str,
    value: String,
}

impl Field {
    fn new(label: &'static str, default: &'static str) -> Self {
        Self {
            label,
            default,
            value: String::new(),
        }
    }

    fn display_value(&self) -> &str {
        if self.value.is_empty() {
            self.default
        } else {
            &self.value
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Focus {
    Platform,
    Provider,
    Field(usize),
}

pub fn run_setup() -> anyhow::Result<()> {
    let existing = load_existing_config();

    let mut platform = match &existing {
        Some(AsrConfig::Whisper(_)) => Platform::Whisper,
        Some(AsrConfig::WebVosk) | None => Platform::Whisper,
    };

    let mut provider = Provider::OpenAI;
    let mut fields = whisper_fields();

    // If existing config, try to detect provider and load values
    if let Some(AsrConfig::Whisper(cfg)) = &existing {
        fields[0].value = cfg.url.clone();
        fields[1].value = cfg.model.clone();
        fields[2].value = cfg.api_key.clone();
        fields[3].value = cfg.lang.clone();
        fields[4].value = cfg.prompt.clone();
        // Detect provider by URL
        for p in Provider::all() {
            let (url, _) = p.defaults();
            if cfg.url == url {
                provider = p;
                break;
            }
        }
    }

    let mut focus = Focus::Platform;
    let mut cursor_pos: usize = 0;
    let mut status_msg: Option<String> = None;

    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = setup_loop(
        &mut terminal,
        &mut platform,
        &mut provider,
        &mut fields,
        &mut focus,
        &mut cursor_pos,
        &mut status_msg,
    );

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn setup_loop(
    terminal: &mut Term,
    platform: &mut Platform,
    provider: &mut Provider,
    fields: &mut [Field],
    focus: &mut Focus,
    cursor_pos: &mut usize,
    status_msg: &mut Option<String>,
) -> anyhow::Result<()> {
    loop {
        terminal.draw(|f| {
            draw_setup(
                f,
                *platform,
                *provider,
                fields,
                *focus,
                *cursor_pos,
                status_msg.as_deref(),
            );
        })?;

        if event::poll(std::time::Duration::from_millis(100))? {
            let evt = event::read()?;
            if let event::Event::Key(key) = evt {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                if status_msg.is_some() {
                    return Ok(());
                }
                match key.code {
                    KeyCode::Esc => match focus {
                        Focus::Platform => return Ok(()),
                        Focus::Provider => *focus = Focus::Platform,
                        Focus::Field(_) => {
                            *focus = Focus::Provider;
                        }
                    },
                    KeyCode::Enter => {
                        if let Focus::Platform = focus {
                            if *platform == Platform::WebVosk {
                                let config = AsrConfig::WebVosk;
                                match save_config(&config) {
                                    Ok(()) => {
                                        *status_msg = Some("Saved! Press any key to exit.".into())
                                    }
                                    Err(e) => *status_msg = Some(format!("Error: {e}")),
                                }
                            } else {
                                *focus = Focus::Provider;
                            }
                        } else if let Focus::Provider = focus {
                            apply_provider_defaults(*provider, fields);
                            *focus = Focus::Field(0);
                            *cursor_pos = fields[0].value.len();
                        } else {
                            let config = build_config(*platform, fields);
                            match save_config(&config) {
                                Ok(()) => {
                                    *status_msg = Some("Saved! Press any key to exit.".into())
                                }
                                Err(e) => *status_msg = Some(format!("Error: {e}")),
                            }
                        }
                    }
                    KeyCode::Up => match focus {
                        Focus::Platform => {}
                        Focus::Provider => {
                            *focus = Focus::Platform;
                        }
                        Focus::Field(0) => {
                            *focus = Focus::Provider;
                        }
                        Focus::Field(i) => {
                            *i -= 1;
                            *cursor_pos = fields[*i].value.len();
                        }
                    },
                    KeyCode::Down => match focus {
                        Focus::Platform => {
                            if *platform == Platform::Whisper {
                                *focus = Focus::Provider;
                            }
                        }
                        Focus::Provider => {
                            *focus = Focus::Field(0);
                            *cursor_pos = fields[0].value.len();
                        }
                        Focus::Field(i) => {
                            if *i + 1 < fields.len() {
                                *i += 1;
                                *cursor_pos = fields[*i].value.len();
                            }
                        }
                    },
                    KeyCode::Left => match focus {
                        Focus::Platform => {
                            let all = Platform::all();
                            let idx = all.iter().position(|p| *p == *platform).unwrap_or(0);
                            if idx > 0 {
                                *platform = all[idx - 1];
                            }
                        }
                        Focus::Provider => {
                            let all = Provider::all();
                            let idx = all.iter().position(|p| *p == *provider).unwrap_or(0);
                            if idx > 0 {
                                *provider = all[idx - 1];
                            }
                        }
                        Focus::Field(_) => {
                            if *cursor_pos > 0 {
                                *cursor_pos -= 1;
                            }
                        }
                    },
                    KeyCode::Right => match focus {
                        Focus::Platform => {
                            let all = Platform::all();
                            let idx = all.iter().position(|p| *p == *platform).unwrap_or(0);
                            if idx + 1 < all.len() {
                                *platform = all[idx + 1];
                            }
                        }
                        Focus::Provider => {
                            let all = Provider::all();
                            let idx = all.iter().position(|p| *p == *provider).unwrap_or(0);
                            if idx + 1 < all.len() {
                                *provider = all[idx + 1];
                            }
                        }
                        Focus::Field(_) => {
                            let field = active_field(fields, focus);
                            let len = if field.value.is_empty() {
                                0
                            } else {
                                field.value.len()
                            };
                            if *cursor_pos < len {
                                *cursor_pos += 1;
                            }
                        }
                    },
                    KeyCode::Tab => {
                        *focus = match focus {
                            Focus::Platform => Focus::Provider,
                            Focus::Provider => Focus::Field(0),
                            Focus::Field(i) => {
                                if *i + 1 < fields.len() {
                                    Focus::Field(*i + 1)
                                } else {
                                    Focus::Platform
                                }
                            }
                        };
                        *cursor_pos = match focus {
                            Focus::Field(j) => {
                                let f = &fields[*j];
                                if f.value.is_empty() { 0 } else { f.value.len() }
                            }
                            _ => 0,
                        };
                    }
                    KeyCode::Backspace => {
                        if let Focus::Field(_) = focus {
                            let field = active_field_mut(fields, focus);
                            if !field.value.is_empty() {
                                let pos = (*cursor_pos).min(field.value.len());
                                if pos > 0 {
                                    field.value.remove(pos - 1);
                                    *cursor_pos = pos - 1;
                                }
                            }
                        }
                    }
                    KeyCode::Char(c) => {
                        if let Focus::Field(_) = focus {
                            let field = active_field_mut(fields, focus);
                            let pos = (*cursor_pos).min(field.value.len());
                            field.value.insert(pos, c);
                            *cursor_pos = pos + 1;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

fn active_field<'a>(fields: &'a [Field], focus: &Focus) -> &'a Field {
    match focus {
        Focus::Field(i) => &fields[*i],
        _ => &fields[0],
    }
}

fn active_field_mut<'a>(fields: &'a mut [Field], focus: &Focus) -> &'a mut Field {
    match focus {
        Focus::Field(i) => &mut fields[*i],
        _ => &mut fields[0],
    }
}

fn whisper_fields() -> Vec<Field> {
    vec![
        Field::new("URL", "https://api.openai.com/v1/audio/transcriptions"),
        Field::new("Model", "whisper-1"),
        Field::new("API Key", ""),
        Field::new("Language", ""),
        Field::new("Prompt", ""),
    ]
}

fn apply_provider_defaults(provider: Provider, fields: &mut [Field]) {
    let (url, model) = provider.defaults();
    if fields[0].value.is_empty() {
        fields[0].default = url;
    }
    if fields[1].value.is_empty() {
        fields[1].default = model;
    }
}

fn build_config(platform: Platform, fields: &[Field]) -> AsrConfig {
    match platform {
        Platform::WebVosk => AsrConfig::WebVosk,
        Platform::Whisper => AsrConfig::Whisper(WhisperASRConfig {
            url: if fields[0].value.is_empty() {
                fields[0].default.to_string()
            } else {
                fields[0].value.clone()
            },
            api_key: fields[2].value.clone(),
            lang: fields[3].value.clone(),
            model: if fields[1].value.is_empty() {
                fields[1].default.to_string()
            } else {
                fields[1].value.clone()
            },
            prompt: fields[4].value.clone(),
        }),
    }
}

fn load_existing_config() -> Option<AsrConfig> {
    let home = dirs::home_dir()?;
    let path = home.join(".vibetty").join("config.toml");
    if !path.exists() {
        return None;
    }
    let content = std::fs::read_to_string(&path).ok()?;
    toml::from_str(&content).ok()
}

fn save_config(config: &AsrConfig) -> anyhow::Result<()> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Cannot find home directory"))?;
    let dir = home.join(".vibetty");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("config.toml");
    let content = toml::to_string_pretty(config)?;
    std::fs::write(&path, content)?;
    Ok(())
}

fn draw_setup(
    f: &mut Frame,
    platform: Platform,
    provider: Provider,
    fields: &[Field],
    focus: Focus,
    cursor_pos: usize,
    status_msg: Option<&str>,
) {
    let size = f.area();

    let outer = Rect::new(
        size.x + 2,
        size.y + 1,
        size.width.saturating_sub(4),
        size.height.saturating_sub(2),
    );
    let block = Block::default()
        .title("  Vibetty ASR Setup  ")
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL);
    f.render_widget(block, outer);

    let inner = outer.inner(ratatui::layout::Margin::new(2, 1));

    // Split into content area (flexible) and footer (fixed at bottom)
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),    // content
            Constraint::Length(3), // footer
        ])
        .split(inner);

    // Footer at the bottom
    let help_text = status_msg
        .map(|s| s.to_string())
        .unwrap_or_else(|| "[Enter] Confirm  [Esc] Cancel  [\u{2190}\u{2192}] Switch".to_string());
    f.render_widget(
        Paragraph::new(help_text)
            .alignment(Alignment::Center)
            .block(Block::new().borders(Borders::ALL)),
        main_chunks[1],
    );

    // Content area
    let show_provider = platform == Platform::Whisper;

    let mut constraints = vec![
        Constraint::Length(1), // Platform label
        Constraint::Length(1), // Platform selector
    ];

    if show_provider {
        constraints.push(Constraint::Length(1)); // Provider label
        constraints.push(Constraint::Length(1)); // Provider selector
    }

    constraints.push(Constraint::Length(1)); // blank

    if show_provider {
        constraints.extend(fields.iter().map(|_| Constraint::Length(1)));
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(main_chunks[0]);

    let mut row = 0;

    // Platform label
    let label_style = if matches!(focus, Focus::Platform) {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    f.render_widget(Paragraph::new("Platform:").style(label_style), chunks[row]);
    row += 1;

    // Platform selector
    let platform_text = Platform::all()
        .iter()
        .map(|p| {
            if *p == platform {
                format!("[ {} ]", p.label())
            } else {
                format!("  {}  ", p.label())
            }
        })
        .collect::<Vec<_>>()
        .join("  ");
    f.render_widget(
        Paragraph::new(platform_text).style(if matches!(focus, Focus::Platform) {
            Style::default()
                .fg(Color::Rgb(180, 120, 255))
                .add_modifier(Modifier::BOLD)
        } else {
            label_style
        }),
        chunks[row],
    );
    row += 1;

    // Provider selector (only for Whisper)
    if show_provider {
        let prov_style = if matches!(focus, Focus::Provider) {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        f.render_widget(Paragraph::new("Provider:").style(prov_style), chunks[row]);
        row += 1;

        let prov_text = Provider::all()
            .iter()
            .map(|p| {
                if *p == provider {
                    format!("[ {} ]", p.label())
                } else {
                    format!("  {}  ", p.label())
                }
            })
            .collect::<Vec<_>>()
            .join("  ");
        f.render_widget(
            Paragraph::new(prov_text).style(if matches!(focus, Focus::Provider) {
                Style::default()
                    .fg(Color::Rgb(180, 120, 255))
                    .add_modifier(Modifier::BOLD)
            } else {
                prov_style
            }),
            chunks[row],
        );
        row += 1;
    }

    // blank
    row += 1;

    // Fields (only show for Whisper)
    if show_provider {
        for (i, field) in fields.iter().enumerate() {
            let is_focused = matches!(focus, Focus::Field(idx) if idx == i);
            let style = if is_focused {
                Style::default().add_modifier(Modifier::UNDERLINED)
            } else {
                Style::default()
            };

            let label_col_width: usize = 12;
            let (display_str, value_style) = if field.value.is_empty() && !field.default.is_empty()
            {
                (field.default.to_string(), style.fg(Color::DarkGray))
            } else {
                (field.display_value().to_string(), style)
            };

            let line = format!(
                "{:width$} {}",
                field.label,
                display_str,
                width = label_col_width
            );
            f.render_widget(Paragraph::new(line).style(value_style), chunks[row]);

            if is_focused {
                let effective_pos = if field.value.is_empty() {
                    0
                } else {
                    cursor_pos
                };
                let cursor_x = chunks[row].x + label_col_width as u16 + 1 + effective_pos as u16;
                let cursor_y = chunks[row].y;
                f.set_cursor_position((cursor_x, cursor_y));
            }

            row += 1;
        }
    }
}
