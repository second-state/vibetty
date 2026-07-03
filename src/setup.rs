//! `vibetty setup` —— TUI 配置 MQTT 传输,写入 `~/.vibetty/config.toml` 的 `[mqtt]` 段。
//!
//! 上下选择字段,Enter 进入编辑,字符直接输入,`s` 保存,`q`/Esc 退出。
//! 保存时把 `[mqtt]` 段写回 config.toml(保留文件里其它段)。

use crossterm::{
    event::{self, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
};
use std::io::Stdout;

use crate::config::MqttConfig;

type Term = Terminal<CrosstermBackend<Stdout>>;

fn config_path() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|h| h.join(".vibetty").join("config.toml"))
}

/// 读取现有 `[mqtt]` 段,用于预填表单。
fn load_mqtt() -> Option<MqttConfig> {
    let path = config_path()?;
    let content = std::fs::read_to_string(&path).ok()?;
    #[derive(serde::Deserialize)]
    struct Section {
        #[serde(default)]
        mqtt: Option<MqttConfig>,
    }
    toml::from_str::<Section>(&content).ok()?.mqtt
}

/// 把 `[mqtt]` 段写回 `~/.vibetty/config.toml`,保留文件中已有的其它段。
fn save_mqtt(mqtt: &MqttConfig) -> anyhow::Result<()> {
    let path = config_path().ok_or_else(|| anyhow::anyhow!("Cannot find home directory"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut table: toml::Table = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default();
    table.insert("mqtt".to_string(), toml::Value::try_from(mqtt)?);
    std::fs::write(&path, toml::to_string_pretty(&table)?)?;
    Ok(())
}

struct Field {
    label: &'static str,
    value: String,
    hint: &'static str,
}

fn fields_from(existing: Option<&MqttConfig>) -> Vec<Field> {
    let e = existing.cloned();
    vec![
        Field {
            label: "enable",
            value: e
                .as_ref()
                .map(|c| c.enable.to_string())
                .unwrap_or_else(|| "true".into()),
            hint: "Master switch: true / false (empty = true)",
        },
        Field {
            label: "host",
            value: e.as_ref().map(|c| c.host.clone()).unwrap_or_default(),
            hint: "Host, or full URL mqtt://[user:pass@]host:port",
        },
        Field {
            label: "port",
            value: e
                .as_ref()
                .map(|c| c.port.to_string())
                .unwrap_or_else(|| "1883".into()),
            hint: "1883 = plaintext, 8883 = TLS (auto)",
        },
        Field {
            label: "client_id",
            value: e.as_ref().map(|c| c.client_id.clone()).unwrap_or_default(),
            hint: "Empty = auto vibetty-{pid}",
        },
        Field {
            label: "use_tls",
            value: e
                .as_ref()
                .and_then(|c| c.use_tls)
                .map(|b| b.to_string())
                .unwrap_or_default(),
            hint: "Empty = auto (8883 on); or true / false",
        },
        Field {
            label: "username",
            value: e
                .as_ref()
                .and_then(|c| c.username.clone())
                .unwrap_or_default(),
            hint: "Broker login; also topic segment 1 (empty = device fingerprint)",
        },
        Field {
            label: "password",
            value: e
                .as_ref()
                .and_then(|c| c.password.clone())
                .unwrap_or_default(),
            hint: "Optional",
        },
        Field {
            label: "qos",
            value: e
                .as_ref()
                .map(|c| c.qos.to_string())
                .unwrap_or_else(|| "1".into()),
            hint: "0 / 1 / 2, default 1",
        },
        Field {
            label: "keep_alive_secs",
            value: e
                .as_ref()
                .map(|c| c.keep_alive_secs.to_string())
                .unwrap_or_else(|| "30".into()),
            hint: "Default 30",
        },
        Field {
            label: "builtin_broker",
            value: e
                .as_ref()
                .map(|c| c.builtin_broker.to_string())
                .unwrap_or_else(|| "false".into()),
            hint: "true = spawn built-in rumqttd here (LAN, anonymous)",
        },
        Field {
            label: "builtin_ws_port",
            value: e
                .as_ref()
                .map(|c| c.builtin_ws_port.to_string())
                .unwrap_or_else(|| "9001".into()),
            hint: "Built-in broker WS port (default 9001)",
        },
    ]
}

fn field<'a>(fields: &'a [Field], label: &str) -> &'a Field {
    fields
        .iter()
        .find(|f| f.label == label)
        .expect("MQTT field always present")
}

/// 由表单字段构建 `MqttConfig`;host 必填(除非 builtin_broker=true),数值/枚举字段非法时报错。
fn mqtt_from_fields(fields: &[Field]) -> anyhow::Result<MqttConfig> {
    let enable = match field(fields, "enable").value.trim() {
        "" | "true" | "1" => true,
        "false" | "0" => false,
        s => anyhow::bail!("invalid enable: {s} (true / false)"),
    };
    let builtin_broker = match field(fields, "builtin_broker").value.trim() {
        "" | "false" | "0" => false,
        "true" | "1" => true,
        s => anyhow::bail!("invalid builtin_broker: {s} (true / false)"),
    };
    let host = field(fields, "host").value.trim().to_string();
    // 内置 broker 模式下 host 会被忽略(改连 127.0.0.1),所以不强制填。
    if enable && !builtin_broker && host.is_empty() {
        anyhow::bail!("host is required when enable=true and builtin_broker=false");
    }
    let port = parse_or(field(fields, "port"), 1883)?;
    let client_id = field(fields, "client_id").value.clone();
    let use_tls = match field(fields, "use_tls").value.trim() {
        "" => None,
        "true" | "1" => Some(true),
        "false" | "0" => Some(false),
        s => anyhow::bail!("invalid use_tls: {s} (empty / true / false)"),
    };
    let username = opt_string(field(fields, "username").value.clone());
    let password = opt_string(field(fields, "password").value.clone());
    let qos = parse_or(field(fields, "qos"), 1)?;
    let keep_alive_secs = parse_or(field(fields, "keep_alive_secs"), 30)?;
    let builtin_ws_port = parse_or(field(fields, "builtin_ws_port"), 9001)?;
    Ok(MqttConfig {
        enable,
        host,
        port,
        client_id,
        use_tls,
        username,
        password,
        qos,
        keep_alive_secs,
        builtin_broker,
        builtin_ws_port,
    })
}

fn opt_string(v: String) -> Option<String> {
    if v.is_empty() { None } else { Some(v) }
}

fn parse_or<T: std::str::FromStr>(fld: &Field, default: T) -> anyhow::Result<T> {
    let raw = fld.value.trim();
    if raw.is_empty() {
        return Ok(default);
    }
    raw.parse::<T>()
        .map_err(|_| anyhow::anyhow!("cannot parse {}: {raw}", fld.label))
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Select,
    Edit,
}

pub fn run_setup() -> anyhow::Result<()> {
    let existing = load_mqtt();
    let mut fields = fields_from(existing.as_ref());
    let mut state = ListState::default();
    state.select(Some(0));
    let mut mode = Mode::Select;
    let mut status: Option<String> = None;

    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = setup_loop(
        &mut terminal,
        &mut fields,
        &mut state,
        &mut mode,
        &mut status,
    );

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn setup_loop(
    terminal: &mut Term,
    fields: &mut [Field],
    state: &mut ListState,
    mode: &mut Mode,
    status: &mut Option<String>,
) -> anyhow::Result<()> {
    loop {
        terminal.draw(|f| draw(f, fields, state, *mode, status.as_deref()))?;

        if !event::poll(std::time::Duration::from_millis(100))? {
            continue;
        }
        let event::Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        // 保存/出错后,任意键退出。
        if status.is_some() {
            return Ok(());
        }

        match mode {
            Mode::Select => match key.code {
                KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                KeyCode::Char('s') => match mqtt_from_fields(fields) {
                    Ok(cfg) => match save_mqtt(&cfg) {
                        Ok(()) => {
                            let p = config_path()
                                .map(|p| p.display().to_string())
                                .unwrap_or_default();
                            *status = Some(format!("Saved to {p}. Press any key to exit."));
                        }
                        Err(e) => *status = Some(format!("Save failed: {e}")),
                    },
                    Err(e) => *status = Some(format!("Invalid input: {e}")),
                },
                KeyCode::Up => state.select_previous(),
                KeyCode::Down => state.select_next(),
                KeyCode::Enter => *mode = Mode::Edit,
                _ => {}
            },
            Mode::Edit => {
                let Some(i) = state.selected() else {
                    *mode = Mode::Select;
                    continue;
                };
                match key.code {
                    KeyCode::Esc | KeyCode::Enter => *mode = Mode::Select,
                    KeyCode::Backspace => {
                        fields[i].value.pop();
                    }
                    KeyCode::Char(c) => fields[i].value.push(c),
                    _ => {}
                }
            }
        }
    }
}

fn draw(f: &mut Frame, fields: &[Field], state: &mut ListState, mode: Mode, status: Option<&str>) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(3),
        ])
        .split(f.area());

    let title = Paragraph::new("Vibetty — MQTT setup  (~/.vibetty/config.toml)")
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, chunks[0]);

    let items: Vec<ListItem<'_>> = fields
        .iter()
        .enumerate()
        .map(|(i, fld)| {
            let sel = state.selected() == Some(i);
            let editing = sel && mode == Mode::Edit;
            let val = if editing {
                format!("{}▌", fld.value)
            } else if fld.value.is_empty() {
                "(empty)".to_string()
            } else {
                fld.value.clone()
            };
            ListItem::new(Line::from(format!(
                " {:<15}: {:<26}  {}",
                fld.label, val, fld.hint
            )))
        })
        .collect();
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title("MQTT fields"))
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶");
    f.render_stateful_widget(list, chunks[1], state);

    let footer = match status {
        Some(s) => s.to_string(),
        None => match mode {
            Mode::Select => " ↑/↓ select  ·  Enter edit  ·  s save  ·  q/Esc quit".to_string(),
            Mode::Edit => " typing edits the field  ·  Enter/Esc done".to_string(),
        },
    };
    f.render_widget(
        Paragraph::new(footer).alignment(Alignment::Center),
        chunks[2],
    );
}
