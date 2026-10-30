use serde::{Deserialize, Serialize};

/// agent 运行状态。来源是 OSC 7501(Program Status Protocol)上报:
/// - working / idle / done / error 直接对应协议状态;
/// - blocked(等用户操作)映射成 Waiting —— 序列化名沿用旧值,ESP32 端语义不变。
///
/// PTY 里的程序不发 7501(普通 shell、未跟进协议的工具)时,状态停在初始 Working,
/// 与旧行为(嗅探不到 → Working)一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentState {
    Working,
    Waiting,
    Idle,
    Done,
    Error,
}

impl AgentState {
    pub fn is_waiting(&self) -> bool {
        matches!(self, AgentState::Waiting)
    }
}

/// 一条 OSC 7501 上报:`ESC ] 7501 ; state=working:app=codex:msg=<base64> ESC \`
/// (spec: <https://mitchellh.com/writing/program-status-osc7501>)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramStatus {
    pub state: AgentState,
    /// blocked 的细分:permission / question / auth(spec:未识别值视为缺失)。
    pub kind: Option<String>,
    /// 程序稳定标识,如 cargo / claude-code(spec 字符集 [A-Za-z0-9_.+-]{1,32})。
    pub app: Option<String>,
    /// working/blocked 的进度 0-100;缺省 = 不确定。
    pub progress: Option<u8>,
    /// 一行人类可读说明(msg= 的 base64 解码结果)。
    pub msg: Option<String>,
}

/// OSC 7501 特性探测的应答字节:`ESC ] 7501 ; ? : states=... : kinds=... ESC \`。
/// vibetty 支持全部五个状态与三种 blocked kind,显式列出(spec:省略等价,列出更明确)。
/// 由 ws 主循环在收到 `7501;?` 探测后写进 PTY 输入侧(callback 拿不到 writer)。
pub const OSC7501_PROBE_REPLY: &[u8] =
    b"\x1b]7501;?:states=idle,working,done,blocked,error:kinds=permission,question,auth\x1b\\";

/// 解析 OSC 7501 body(`;` 之后那段,如 `state=blocked:kind=permission:msg=...`)。
/// 按 spec:malformed pair 跳过、未知 key 忽略、同 key 最后一个生效;
/// state 缺失或无法识别 → None(整条丢弃,防止未来新状态被误当别的);
/// msg base64 解不开、非 UTF-8 或解码后含控制字符 → None(整条丢弃)。
pub fn parse_osc7501(body: &[u8]) -> Option<ProgramStatus> {
    let mut state: Option<AgentState> = None;
    let mut kind: Option<String> = None;
    let mut app: Option<String> = None;
    let mut progress: Option<u8> = None;
    let mut msg_b64: Option<&[u8]> = None;

    for pair in body.split(|&b| b == b':') {
        // 手动按第一个 '=' 切(slice::split_once 尚未 stable)
        let Some(eq) = pair.iter().position(|&b| b == b'=') else {
            continue; // 没有 '=' → malformed,跳过
        };
        let (k, v) = (&pair[..eq], &pair[eq + 1..]);
        let k = k.trim_ascii();
        let v = v.trim_ascii();
        if k.is_empty() || v.is_empty() {
            continue;
        }
        match k {
            b"state" => state = parse_state(v),
            b"kind" => {
                kind = match v {
                    b"permission" | b"question" | b"auth" => {
                        Some(String::from_utf8_lossy(v).into_owned())
                    }
                    _ => None,
                }
            }
            b"app" => {
                // spec 字符集 [A-Za-z0-9_.+-]{1,32},不符视为缺失
                let ok = (1..=32).contains(&v.len())
                    && v.iter().all(|c| {
                        c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'+' | b'-')
                    });
                if ok {
                    app = Some(String::from_utf8_lossy(v).into_owned());
                } else {
                    app = None;
                }
            }
            b"progress" => {
                progress = std::str::from_utf8(v)
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .filter(|p| *p <= 100)
            }
            b"msg" => msg_b64 = Some(v),
            _ => {} // 未知 key 忽略(协议扩展位)
        }
    }

    let state = state?;

    // msg:base64( padding 可选)→ UTF-8 → 不含控制字符,任一不满足整条丢弃(spec)。
    let msg = match msg_b64 {
        Some(b64) => {
            let decoded = base64_decode(b64)?;
            let s = String::from_utf8(decoded).ok()?;
            if s.bytes().any(|b| b < 0x20 || b == 0x7f) {
                return None;
            }
            Some(s)
        }
        None => None,
    };

    Some(ProgramStatus {
        state,
        kind,
        app,
        progress,
        msg,
    })
}

/// 协议五个状态 → AgentState。未识别的值返回 None(整条报告作废)。
fn parse_state(v: &[u8]) -> Option<AgentState> {
    match v {
        b"working" => Some(AgentState::Working),
        b"blocked" => Some(AgentState::Waiting),
        b"idle" => Some(AgentState::Idle),
        b"done" => Some(AgentState::Done),
        b"error" => Some(AgentState::Error),
        _ => None,
    }
}

/// 标准 base64 解码(padding 可选)。含非法字符或长度不合法 → None。
/// 只在这一处用,不值得为此给 vibetty 加 base64 直接依赖。
fn base64_decode(input: &[u8]) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some((c - b'A') as u32),
            b'a'..=b'z' => Some((c - b'a' + 26) as u32),
            b'0'..=b'9' => Some((c - b'0' + 52) as u32),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    // 去掉 padding 后必须是 2/3/4 字节的倍数长度(1 mod 4 非法)
    let trimmed: Vec<u8> = input.iter().copied().filter(|&c| c != b'=').collect();
    if input.iter().any(|&c| c != b'=' && val(c).is_none()) || trimmed.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(trimmed.len() * 3 / 4);
    for chunk in trimmed.chunks(4) {
        let mut acc = 0u32;
        for &c in chunk {
            acc = (acc << 6) | val(c)?;
        }
        match chunk.len() {
            4 => out.extend_from_slice(&[(acc >> 16) as u8, (acc >> 8) as u8, acc as u8]),
            3 => out.extend_from_slice(&[(acc >> 10) as u8, (acc >> 2) as u8]),
            2 => out.push((acc >> 4) as u8),
            _ => unreachable!("chunks(4) 只产出 2/3/4,1 已被长度检查排除"),
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn working_minimal() {
        let s = parse_osc7501(b"state=working:app=codex").unwrap();
        assert_eq!(s.state, AgentState::Working);
        assert_eq!(s.app.as_deref(), Some("codex"));
        assert_eq!(s.kind, None);
        assert_eq!(s.progress, None);
        assert_eq!(s.msg, None);
    }

    #[test]
    fn blocked_maps_to_waiting_with_kind() {
        let s = parse_osc7501(b"state=blocked:kind=permission").unwrap();
        assert_eq!(s.state, AgentState::Waiting);
        assert!(s.state.is_waiting());
        assert_eq!(s.kind.as_deref(), Some("permission"));
    }

    #[test]
    fn kind_unrecognized_treated_as_absent() {
        let s = parse_osc7501(b"state=blocked:kind=frobnicate").unwrap();
        assert_eq!(s.kind, None);
    }

    #[test]
    fn full_report_with_base64_msg() {
        // "Apply 3 to add, 1 to change, 0 to destroy?"
        let msg = "QXBwbHkgMyB0byBhZGQsIDEgdG8gY2hhbmdlLCAwIHRvIGRlc3Ryb3k/";
        let body = format!("state=blocked:kind=permission:app=terraform:msg={msg}").into_bytes();
        let s = parse_osc7501(&body).unwrap();
        assert_eq!(s.state, AgentState::Waiting);
        assert_eq!(s.app.as_deref(), Some("terraform"));
        assert_eq!(
            s.msg.as_deref(),
            Some("Apply 3 to add, 1 to change, 0 to destroy?")
        );
    }

    #[test]
    fn msg_without_padding_ok() {
        // "hi" → "aGk=";无 padding "aGk" 也接受
        assert_eq!(
            parse_osc7501(b"state=idle:msg=aGk").unwrap().msg.as_deref(),
            Some("hi")
        );
        assert_eq!(
            parse_osc7501(b"state=idle:msg=aGk=")
                .unwrap()
                .msg
                .as_deref(),
            Some("hi")
        );
    }

    #[test]
    fn unknown_state_discards_whole_report() {
        assert_eq!(parse_osc7501(b"state=frobnicated"), None);
        assert_eq!(parse_osc7501(b"app=codex"), None); // 缺 state
    }

    #[test]
    fn malformed_pairs_skipped() {
        let s = parse_osc7501(b"state=done:nopairs:app=").unwrap();
        assert_eq!(s.state, AgentState::Done);
        assert_eq!(s.app, None); // 空值 → 跳过
    }

    #[test]
    fn duplicate_key_last_wins() {
        let s = parse_osc7501(b"state=working:state=done").unwrap();
        assert_eq!(s.state, AgentState::Done);
    }

    #[test]
    fn bad_msg_discards_whole_report() {
        assert_eq!(parse_osc7501(b"state=done:msg=!!!"), None); // 非法 base64
        // 解码后含换行(控制字符)→ 整条丢弃
        assert_eq!(parse_osc7501(b"state=done:msg=Cg=="), None); // "\n"
    }

    #[test]
    fn progress_range() {
        assert_eq!(
            parse_osc7501(b"state=working:progress=42")
                .unwrap()
                .progress,
            Some(42)
        );
        assert_eq!(
            parse_osc7501(b"state=working:progress=101")
                .unwrap()
                .progress,
            None
        );
        assert_eq!(
            parse_osc7501(b"state=working:progress=-1")
                .unwrap()
                .progress,
            None
        );
    }

    #[test]
    fn app_charset_enforced() {
        assert!(parse_osc7501(b"state=idle:app=claude-code_v2.1").is_some());
        // 非法字符 / 超长 → 视为缺失(报告本身有效)
        let s = parse_osc7501(b"state=idle:app=bad$app").unwrap();
        assert_eq!(s.app, None);
    }

    #[test]
    fn base64_decode_basics() {
        assert_eq!(base64_decode(b"aGk=").unwrap(), b"hi");
        assert_eq!(base64_decode(b"aGk").unwrap(), b"hi");
        assert_eq!(base64_decode(b"").unwrap(), b"");
        assert_eq!(base64_decode(b"AAAA").unwrap(), [0, 0, 0]);
        assert_eq!(base64_decode(b"A"), None); // 1 mod 4
        assert_eq!(base64_decode(b"A@A="), None); // 非法字符
    }
}
