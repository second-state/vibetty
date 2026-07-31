//! `vibetty herdr <target>` 模式:attach 到一个 herdr agent 终端,经 MQTT 分享。
//!
//! 这个子命令跑在 herdr 分出的 **1 行高 pane** 里,不是终端查看器:PTY 里跑
//! `herdr agent attach <target>`(80×40),流接进 vt_parser → MQTT 分享照常,但
//! **本地不渲染 PTY 内容**,TUI 只占那 1 行显示状态(见 `ui.rs`)。本地按键不转发
//! 进 PTY,只有 `Ctrl+C` / `q` 退出;远端按键仍走 MQTT → PTY。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use tokio::sync::{broadcast, mpsc};

use crate::broker;
use crate::config::MqttConfig;
use crate::mqtt;
use crate::protocol::{ClientMessage, ServerMessage};

pub mod ui;

/// herdr 插件 id(`herdr-plugin.toml` 里的 `id`),`share` action 用它开 plugin pane。
const HERDR_PLUGIN_ID: &str = "vibetty";
/// `[[panes]]` entrypoint id(跑 `vibetty herdr` 的那个 1 行状态条 pane)。
const HERDR_ENTRYPOINT: &str = "herdr-share";
/// 父 action(`share-herdr`)把要分享的 agent pane id 通过这个 env 传给被开出来的 pane。
pub const HERDR_TARGET_ENV: &str = "VIBETTY_HERDR_TARGET";

/// 终端截图里单个字符单元格的像素宽(`vibetty-screenshot` font_size=14.0 实测 8)。
/// 远端 Sync 在 `pixels=true` 时发像素,要除以它换算列数。
/// ⚠️ 与 `ws.rs` 的同名常量保持一致;改字体/字号都要同步。
pub(crate) const SCREEN_CHAR_WIDTH: u32 = 8;
/// 单个字符单元格的像素高(同上,18)。
pub(crate) const SCREEN_CHAR_HEIGHT: u32 = 18;
/// 截图四周留白(每边像素数),来自 `ScreenshotConfig::default().padding`。
pub(crate) const SCREEN_PADDING: u32 = 16;

type ServerTx = broadcast::Sender<ServerMessage>;

fn send_screen(tx: &ServerTx, screen: Arc<vt100::Screen>) {
    // 广播整张屏幕;渲染成 JPEG 还是 ANSI 文本由 MQTT 端按 image_format 决定。
    let _ = tx.send(ServerMessage::Screen(screen));
}

/// 主事件循环的事件来源(精简:PTY 输出 / MQTT 客户端消息 / 本地 UI 事件)。
enum TerminalEvent {
    Input(ClientMessage),
    InputClosed,
    Ui(ui::HerdrUiEvent),
    PtyOutput(String),
    Error,
}

/// boot 时的 MQTT auto-start:`enable` 起 client、`builtin_broker` 起内置 broker。
/// 返回 (broker 是否在跑, client handle, broker alive 句柄, screen 字节累计计数器)。
/// 没 spawn client 时计数器是空的 Arc(UI 显示 0.00 MB)。
fn autostart_mqtt(
    mqtt_cfg: &Option<MqttConfig>,
    cli_tx: &mpsc::Sender<ClientMessage>,
    tx: &ServerTx,
    image_format: crate::protocol::OutputFormat,
) -> (
    bool,
    Option<mqtt::MqttHandle>,
    Option<Arc<AtomicBool>>,
    Arc<AtomicU64>,
) {
    let Some(cfg) = mqtt_cfg else {
        return (false, None, None, Arc::new(AtomicU64::new(0)));
    };
    let mut broker_on = false;
    let mut broker_alive = None;
    if cfg.builtin_broker {
        let alive = Arc::new(AtomicBool::new(true));
        match broker::spawn_builtin(cfg, alive.clone()) {
            Ok(()) => {
                broker_on = true;
                broker_alive = Some(alive);
                log::info!("[mqtt] broker auto-started on :{}", cfg.builtin_port);
            }
            Err(e) => log::warn!("[mqtt] broker auto-start failed: {e}"),
        }
    }
    let (client, screen_bytes) = if cfg.enable {
        log::info!("[mqtt] client auto-started");
        let (h, stats) = mqtt::spawn(cfg.for_client(), cli_tx.clone(), tx.clone(), image_format);
        (Some(h), stats)
    } else {
        (None, Arc::new(AtomicU64::new(0)))
    };
    (broker_on, client, broker_alive, screen_bytes)
}

/// 解析 herdr 二进制路径:优先 `$HERDR_BIN_PATH`(herdr 调插件时注入),否则 PATH 里的 `herdr`。
fn herdr_bin() -> String {
    std::env::var("HERDR_BIN_PATH")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "herdr".to_string())
}

/// `herdr agent get <target>` 的解析结果(只取关心的字段,其余忽略)。
#[derive(Debug, Clone, serde::Deserialize)]
struct HerdrAgentInfo {
    /// agent 类型名(claude / codex / ...),非 agent pane 时为 None。
    #[serde(default)]
    agent: Option<String>,
    /// idle / working / blocked / done / unknown。
    agent_status: String,
    pane_id: String,
    terminal_id: String,
    #[serde(default)]
    terminal_title: Option<String>,
}

/// 直接 shell 出 `herdr agent get <target>` 并自己解析(不走 herdr-plugin SDK,避开它把
/// `Pane.agent_status` 当裸字符串之类的反序列化坑)。target 是 pane id(如 `wA:p8`)。
async fn herdr_agent_get(target: &str) -> anyhow::Result<HerdrAgentInfo> {
    use anyhow::Context as _;
    #[derive(serde::Deserialize)]
    struct Envelope {
        result: ResultBody,
    }
    #[derive(serde::Deserialize)]
    struct ResultBody {
        agent: HerdrAgentInfo,
    }

    let output = tokio::process::Command::new(herdr_bin())
        .args(["agent", "get", target])
        .output()
        .await
        .with_context(|| format!("failed to spawn `herdr agent get {target}`"))?;
    if !output.status.success() {
        anyhow::bail!(
            "`herdr agent get {target}` failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let env: Envelope = serde_json::from_slice(&output.stdout).with_context(|| {
        format!(
            "parse `herdr agent get {target}` output: {}",
            String::from_utf8_lossy(&output.stdout)
        )
    })?;
    Ok(env.result.agent)
}

/// 查 target 的 agent 状态 + terminal title + agent 名(经 herdr_agent_get)。
/// 返回 (status, title, agent),agent 形如 "claude"/"codex",非 agent pane 时为 "?"。
async fn get_agent_status(
    target: &str,
) -> anyhow::Result<(herdr_plugin::AgentStatus, String, String)> {
    let info = herdr_agent_get(target).await?;
    log::debug!(
        "[herdr] target {target}: pane={} terminal={} title={:?} agent={:?} status={}",
        info.pane_id,
        info.terminal_id,
        info.terminal_title,
        info.agent,
        info.agent_status
    );
    use herdr_plugin::AgentStatus;
    let status = match info.agent_status.as_str() {
        "working" => AgentStatus::Working,
        "idle" => AgentStatus::Idle,
        "blocked" => AgentStatus::Blocked,
        "done" => AgentStatus::Done,
        _ => AgentStatus::Unknown,
    };
    Ok((
        status,
        info.terminal_title.unwrap_or_default(),
        info.agent.unwrap_or_else(|| "?".to_string()),
    ))
}

/// herdr 的 `AgentStatus` → vibetty 的 `AgentState`:Working 保持 Working,
/// 其余(Idle/Blocked/Done/Unknown)视作 Waiting(agent 在等用户)。
/// ESP32 据此判断是否需要推屏 / 提醒。
fn agent_status_to_state(s: herdr_plugin::AgentStatus) -> crate::terminal::agent::AgentState {
    use herdr_plugin::AgentStatus;
    match s {
        AgentStatus::Working => crate::terminal::agent::AgentState::Working,
        _ => crate::terminal::agent::AgentState::Waiting,
    }
}

/// 读 pane 在当前 tab 布局里的高度(rows)。找不到返回 None。
async fn pane_height(
    client: &herdr_plugin::HerdrClient,
    pane_id: &str,
) -> anyhow::Result<Option<u64>> {
    let layout = client
        .pane()
        .layout(herdr_plugin::PaneSelector::Pane(pane_id.to_owned()))
        .await?;
    Ok(layout
        .layout
        .panes
        .iter()
        .find(|p| p.pane_id == pane_id)
        .map(|p| p.rect.height))
}

/// 把 `pane` 缩到约 1 行:每轮读它的高度并 resize 一步,直到 ≤1 行 / 不再下降 / 上限 30 轮。
///
/// herdr 的 resize 方向语义没文档(实测 `Direction::Down` 缩的是被 resize 的那个 pane),所以
/// 不假设方向 —— 依次试 Up、Down:哪个方向能让本 pane 高度下降就用哪个;方向不对时高度不降
/// (甚至涨),`h >= last` 即换下一个方向。amount 单位也不明,用默认步长 `None` 逐轮逼近。
async fn shrink_pane_to_one_row(
    client: &herdr_plugin::HerdrClient,
    pane: &str,
) -> anyhow::Result<()> {
    use herdr_plugin::Direction;
    'dir: for dir in [Direction::Up, Direction::Down] {
        let mut last = u64::MAX;
        for _ in 0..30 {
            let Some(h) = pane_height(client, pane).await? else {
                return Ok(()); // pane 没了
            };
            if h <= 1 {
                return Ok(()); // 到 1 行
            }
            if h >= last {
                continue 'dir; // 这个方向没让它下降 → 换方向
            }
            last = h;
            let _ = client
                .pane()
                .resize(dir, None, herdr_plugin::PaneSelector::Pane(pane.to_owned()))
                .await;
        }
    }
    Ok(())
}

/// `vibetty share-herdr`:herdr `share` action 入口(OneShotRuntime)。拿当前 focused 的 agent
/// pane,在它下面开一个 plugin pane 跑 `vibetty herdr`(target 走 env),再把新 pane 缩到 ~1 行。
/// 由 herdr 触发(快捷键 / 命令面板),不手动跑。
pub async fn run_share_herdr() -> anyhow::Result<()> {
    use herdr_plugin::{
        App, Context, OneShotRuntime, PluginPaneDirection, PluginPaneOpenOptions,
        PluginPanePlacement,
    };
    App::builder()
        .runtime(OneShotRuntime::new())
        .build()?
        .setup(|ctx: Context| async move {
            let client = ctx.client();
            // 要分享的是触发 action 时 focused 的 pane(HERDR_PANE_ID 是 herdr 临时开起来跑
            // 本命令的 pane,不是用户那个)。缺失才回退 current。
            let target = match ctx
                .env()
                .plugin_context
                .as_ref()
                .and_then(|c| c.focused_pane_id.clone())
            {
                Some(id) => id,
                None => {
                    client
                        .pane()
                        .current(herdr_plugin::PaneSelector::Current)
                        .await?
                        .pane
                        .pane_id
                }
            };
            // 在 target 下面开 plugin pane(跑 vibetty herdr),target 经 env 传进去。
            let resp = client
                .pane()
                .open_plugin_pane(PluginPaneOpenOptions {
                    plugin_id: HERDR_PLUGIN_ID.to_owned(),
                    entrypoint: HERDR_ENTRYPOINT.to_owned(),
                    placement: Some(PluginPanePlacement::Split),
                    workspace_id: None,
                    target_pane_id: Some(target.clone()),
                    direction: Some(PluginPaneDirection::Down),
                    cwd: None,
                    focus: false,
                    env: vec![(HERDR_TARGET_ENV.to_owned(), target.clone())],
                })
                .await?;
            let new_pane = resp.plugin_pane.pane.pane_id;
            // 把新开的 vibetty pane 缩到 ~1 行(直接缩它自己,不碰 agent pane)。
            shrink_pane_to_one_row(client, &new_pane).await?;
            Ok(())
        })
        .run()
        .await?;
    Ok(())
}

/// `vibetty herdr <target>`:自包含 —— 自己 init TUI、起事件循环、建 channel、跑主循环。
pub async fn run_herdr(
    target: String,
    mqtt_cfg: Option<MqttConfig>,
    image_format: crate::protocol::OutputFormat,
    auto_submit: bool,
) -> anyhow::Result<()> {
    // 进 herdr 模式第一件事:查一下 target 的 agent 状态 + terminal title + agent 名
    // (直接 shell 出 `herdr agent get`),记录到日志。
    let (agent_status, terminal_title, agent_name) = get_agent_status(&target).await?;

    // 后台每 1s 轮询 target 的 agent 状态;变化时经 watch 通道通知主循环。
    // (agent 名基本恒定,只在 init 取一次;title 跟着 status 一起走 watch 给 presence 用。)
    let (tx, mut agent_status_rx) = tokio::sync::watch::channel((agent_status, terminal_title));
    let status_target = target.clone();
    tokio::spawn(async move {
        let mut current = agent_status;
        loop {
            match get_agent_status(&status_target).await {
                Ok((s, title, _agent)) if s != current => {
                    current = s;
                    log::info!("[herdr] {status_target} status -> {s:?}");
                    if tx.send((s, title)).is_err() {
                        return; // 主循环退出,receiver 没了。
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    log::warn!("[herdr] status poll for {status_target} failed: {e}");
                    return;
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    });

    // herdr attach:PTY 里跑 `herdr agent attach <target>`(初始 80×40,等远端 sync 再调)。
    let command = [
        "herdr".to_string(),
        "agent".to_string(),
        "attach".to_string(),
        target.clone(),
    ];
    let process_command = command.first().unwrap().as_str();

    let vt_cols = 80;
    let vt_rows = 40;
    let env: [(String, String); 0] = [];
    let mut terminal = crate::terminal::pty::new_with_command(
        process_command,
        &command[1..],
        &env,
        (vt_rows, vt_cols),
    )
    .await?;

    let mut vt_parser = vt100::Parser::new(vt_rows, vt_cols, 1024);

    // 本地 1 行 TUI + 事件循环(只 Ctrl+C/q 退出 + Resize)。
    let mut tui = ui::init_terminal().expect("Failed to initialize terminal");
    let (ui_tx, mut ui_rx) = mpsc::channel(64);
    ui::spawn_event_loop(ui_tx);

    // MQTT 控制消息入口 + 服务端广播源。
    let (cli_tx, mut rx) = tokio::sync::mpsc::channel(1024);
    let (tx, broadcast_rx) = tokio::sync::broadcast::channel(1024);
    drop(broadcast_rx);

    let (mut mqtt_broker_on, mqtt_client, broker_alive, screen_bytes) =
        autostart_mqtt(&mqtt_cfg, &cli_tx, &tx, image_format);

    // 画一次初始状态条(title 此时 ui_title 还没初始化,从 watch 通道取)。
    let init_title = agent_status_rx.borrow().1.clone();
    let _ = ui::draw_status(
        &mut tui,
        &agent_name,
        &target,
        mqtt_client.is_some(),
        screen_bytes.load(Ordering::Relaxed),
        &init_title,
    );

    // presence 心跳:interval 首次 tick 立即返回 → 充当上线公告。
    let mut presence_interval =
        tokio::time::interval(std::time::Duration::from_secs(mqtt::PRESENCE_INTERVAL_SECS));

    // ── 启动初始化去抖:等 PTY 输出静默满 500ms(上限 5s),稳定后发首帧 + presence。
    //   (本地无屏幕可看,期间只吃 PTY 输出进 vt_parser,不 redraw。)
    const INIT_SETTLE: std::time::Duration = std::time::Duration::from_millis(500);
    const INIT_MAX_WAIT: std::time::Duration = std::time::Duration::from_secs(5);
    let init_started = tokio::time::Instant::now();
    let mut last_pty_at = tokio::time::Instant::now();
    'init: loop {
        let settle_deadline = last_pty_at + INIT_SETTLE;
        let max_deadline = init_started + INIT_MAX_WAIT;
        tokio::select! {
            result = terminal.read_pty_output() => match result {
                Ok(output) if !output.is_empty() => {
                    vt_parser.process(output.as_bytes());
                    last_pty_at = tokio::time::Instant::now();
                }
                Ok(_) => break 'init, // 空 = PTY EOF(子进程秒退),交给主 loop 处理。
                Err(e) => {
                    log::error!("[{}] init PTY read error: {:?}", terminal.session_id(), e);
                    break 'init;
                }
            },
            _ = tokio::time::sleep_until(settle_deadline) => break 'init,
            _ = tokio::time::sleep_until(max_deadline) => break 'init,
        }
    }
    let screen = Arc::new(vt_parser.screen().clone());
    send_screen(&tx, screen);
    // 当前 agent 状态(由后台轮询 herdr 的 agent_status 驱动,见主循环 select 的 changed 分支)。
    // 先用 watch 通道里的最新值初始化(可能已在 init settle 期间被轮询刷新)。
    let (agent_status, title) = agent_status_rx.borrow().clone();
    let mut current_state = agent_status_to_state(agent_status);
    let mut ui_title = title.clone();
    let _ = tx.send(ServerMessage::Presence {
        title,
        state: current_state,
    });
    log::info!(
        "[{}] init settled after {}ms, sent first screen + presence",
        terminal.session_id(),
        init_started.elapsed().as_millis()
    );

    // screen 去抖(无条件尾部去抖 100ms)。close=true 时暂停自主推屏。
    const SCREEN_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(100);
    let mut pending_screen: Option<Arc<vt100::Screen>> = None;
    let mut send_deadline: Option<tokio::time::Instant> = None;
    let mut screen_closed = false;

    // resize 后 PTY 重绘 burst 吸收:停顿满 500ms 再发一帧全屏。
    const RESIZE_SETTLE: std::time::Duration = std::time::Duration::from_millis(500);
    let mut resize_settle_until: Option<tokio::time::Instant> = None;

    loop {
        // 内置 broker 可能在后台悄悄退出(端口被占等)。alive 置 false 后翻状态 + 重画状态条。
        if let Some(a) = broker_alive.as_ref()
            && mqtt_broker_on
            && !a.load(Ordering::Relaxed)
        {
            mqtt_broker_on = false;
            log::warn!("[mqtt] builtin broker exited; marking broker off");
            let _ = ui::draw_status(
                &mut tui,
                &agent_name,
                &target,
                mqtt_client.is_some(),
                screen_bytes.load(Ordering::Relaxed),
                &ui_title,
            );
        }

        let deadline_copy = send_deadline;
        let settle_copy = resize_settle_until;
        // biased:入站 MQTT 控制排在 PTY 输出前,狂输出时仍能响应 sync/pty_in/close。
        let event = tokio::select! {
            biased;
            msg = rx.recv() => match msg {
                Some(input) => TerminalEvent::Input(input),
                None => TerminalEvent::InputClosed,
            },
            ui_evt = ui_rx.recv() => match ui_evt {
                Some(evt) => TerminalEvent::Ui(evt),
                None => TerminalEvent::Error,
            },
            // 后台轮询发现 herdr agent 状态变化 → 更新 current_state 并立即重发 presence
            // (取代旧的 title 翻转检测)。
            res = agent_status_rx.changed() => {
                if res.is_ok() {
                    let (s, title) = agent_status_rx.borrow().clone();
                    current_state = agent_status_to_state(s);
                    ui_title = title.clone();
                    log::info!("[herdr] {target} agent status -> {s:?} (state={current_state:?})");
                    let _ = tx.send(ServerMessage::Presence {
                        title,
                        state: current_state,
                    });
                    // 状态/title 变了 → 重画状态条(顺带刷新 MB 计数)。
                    let _ = ui::draw_status(
                        &mut tui,
                        &agent_name,
                        &target,
                        mqtt_client.is_some(),
                        screen_bytes.load(Ordering::Relaxed),
                        &ui_title,
                    );
                } else {
                    // 轮询任务退出(sender 没了);状态保持最后一次。
                    log::debug!("[herdr] agent_status_rx closed");
                }
                continue;
            },
            result = terminal.read_pty_output() => match result {
                Ok(r) => TerminalEvent::PtyOutput(r),
                Err(e) => {
                    log::error!("[{}] Error reading PTY output: {:?}", terminal.session_id(), e);
                    TerminalEvent::Error
                }
            },
            _ = presence_interval.tick() => {
                let _ = tx.send(ServerMessage::Presence {
                    title: ui_title.clone(),
                    state: current_state,
                });
                continue;
            },
            // screen 去抖到期:发最新 pending screen。
            _ = async {
                match deadline_copy {
                    Some(d) => tokio::time::sleep_until(d).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                if let Some(screen) = pending_screen.take() {
                    send_screen(&tx, screen);
                }
                send_deadline = None;
                continue;
            },
            // resize settle 到期:发一帧全屏,退出 settle。
            _ = async {
                match settle_copy {
                    Some(d) => tokio::time::sleep_until(d).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                resize_settle_until = None;
                let screen = Arc::new(vt_parser.screen().clone());
                send_screen(&tx, screen);
                continue;
            },
        };

        match event {
            TerminalEvent::PtyOutput(output) => {
                // 空 = EOF / 子进程退出 → 退出循环。
                if output.is_empty() {
                    log::info!(
                        "[{}] PTY closed (child exited), shutting down",
                        terminal.session_id()
                    );
                    break;
                }
                log::debug!(
                    "[{}] PTY output len: {} bytes (screen={} sb={})",
                    terminal.session_id(),
                    output.len(),
                    if vt_parser.screen().alternate_screen() {
                        "alt"
                    } else {
                        "main"
                    },
                    vt_parser.screen().scrollback(),
                );
                vt_parser.process(output.as_bytes());

                if resize_settle_until.is_some() {
                    // resize 后重绘 burst:吸收,重设 500ms 计时器。
                    resize_settle_until = Some(tokio::time::Instant::now() + RESIZE_SETTLE);
                } else if !screen_closed {
                    // text 模式:实时广播原始 PTY 字节。
                    if image_format.is_text() {
                        let _ = tx.send(ServerMessage::PtyOutput(output.clone().into_bytes()));
                    }
                    // jpeg 模式:入队 pending + 100ms 去抖。
                    if !image_format.is_text()
                        && (vt_parser.screen().scrollback() == 0 || current_state.is_waiting())
                    {
                        pending_screen = Some(Arc::new(vt_parser.screen().clone()));
                        send_deadline = Some(tokio::time::Instant::now() + SCREEN_DEBOUNCE);
                    }
                }
            }

            TerminalEvent::Ui(ui::HerdrUiEvent::Quit) => {
                log::info!("[herdr] quit requested");
                break;
            }
            TerminalEvent::Ui(ui::HerdrUiEvent::Resize) => {
                // 本地 1 行 pane 尺寸变化:不动 PTY(尺寸由远端 sync 驱动),状态条不重画。
            }

            TerminalEvent::Input(ClientMessage::Sync {
                width,
                height,
                pixels,
                close,
            }) => {
                screen_closed = close;
                // pixels=true:像素换算 cols×rows;false:已是字符列/行。
                let (cols, rows) = if pixels {
                    let avail_w = (width as u32).saturating_sub(2 * SCREEN_PADDING);
                    let avail_h = (height as u32).saturating_sub(2 * SCREEN_PADDING);
                    (
                        (avail_w / SCREEN_CHAR_WIDTH).max(8) as u16,
                        (avail_h / SCREEN_CHAR_HEIGHT).max(2) as u16,
                    )
                } else {
                    (width.max(8), height.max(2))
                };
                let (cur_rows, cur_cols) = vt_parser.screen().size();
                let resized = cur_cols != cols || cur_rows != rows;
                if resized {
                    log::debug!(
                        "Sync: {width}×{height}{} -> resize PTY {cur_cols}×{cur_rows} -> {cols}×{rows}",
                        if pixels { "px" } else { "cells" }
                    );
                    vt_parser.screen_mut().set_size(rows, cols);
                    let _ = terminal.resize(rows, cols);
                }
                let screen = Arc::new(vt_parser.screen().clone());
                if close {
                    resize_settle_until = None;
                    pending_screen = None;
                    send_deadline = None;
                } else {
                    send_screen(&tx, screen);
                    if resized {
                        resize_settle_until = Some(tokio::time::Instant::now() + RESIZE_SETTLE);
                        pending_screen = None;
                        send_deadline = None;
                    }
                }
            }

            TerminalEvent::Input(ClientMessage::ScrollUp { rows }) => {
                // 全屏 TUI 的 scrollback 不适用:直接发 PageUp,交给 app 自己滚历史。
                log::debug!("ScrollUp rows={rows} -> send PageUp to PTY");
                terminal.send_bytes(b"\x1b[5~").await?;
            }
            TerminalEvent::Input(ClientMessage::ScrollDown { rows }) => {
                log::debug!("ScrollDown rows={rows} -> send PageDown to PTY");
                terminal.send_bytes(b"\x1b[6~").await?;
            }
            TerminalEvent::Input(ClientMessage::PtyInput(input)) => {
                log::debug!(
                    "Sending input to terminal: {:?}",
                    String::from_utf8_lossy(&input)
                );
                terminal.send_bytes(&input).await?;
            }
            TerminalEvent::Input(ClientMessage::Input(text)) => {
                log::debug!("Sending text input to terminal: {:?}", text);
                terminal.send_text(&text).await?;
                if auto_submit {
                    terminal.send_enter().await?;
                }
            }

            TerminalEvent::InputClosed | TerminalEvent::Error => {
                log::error!("Input channel closed or error occurred, terminating");
                break;
            }
        }
    }

    // 退出前杀掉内层 PTY 子进程(herdr agent attach)。否则子进程还活着、master 端读不到
    // EOF,PTY reader(spawn_blocking 线程)会卡在阻塞 read() 里;tokio 关 runtime 时要等
    // spawn_blocking 线程退出 → 进程挂住退不掉。(PTY 模式靠 Ctrl+C 让子进程自己退;herdr
    // 的 q/Ctrl+C 不转发,必须显式 kill。)子进程已退(EOF 路径)时 kill 是无副作用空操作。
    let _ = terminal.kill().await;

    ui::cleanup_terminal(&mut tui).ok();
    Ok(())
}
