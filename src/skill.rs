//! `vibetty skill install/uninstall` —— 把内置 run-vibetty SKILL.md 写入 / 移除
//! agent 的用户级 skills 目录。
//!
//! - Claude Code: `~/.claude/skills/run-vibetty/SKILL.md`
//! - Codex USER : `~/.agents/skills/run-vibetty/SKILL.md`
//!   (见 developers.openai.com/codex/skills;旧的 `~/.codex/prompts/` 已废弃)
//!
//! 两个 agent 的 SKILL.md 格式完全一致(name + description frontmatter + 渐进披露正文),
//! 仓库内只内嵌一份,按 `--claude` / `--codex` 写到对应目录。
//!
//! 版本感知:install 前先比 `env!("CARGO_PKG_VERSION")` 与目标目录下伴生文件
//! `.vibetty-version`,同版本则跳过(避免无谓覆盖),不同则覆盖升级。

use std::path::PathBuf;

use anyhow::{Context, anyhow, bail};

use crate::config::SkillAction;

/// 内嵌的 SKILL.md(沿用 `static_page.rs` 里 `include_str!` 的约定)。
const SKILL_MD: &str = include_str!("../resources/skills/run-vibetty/SKILL.md");

/// 当前 vibetty 版本(编译期从 Cargo.toml 注入);写入 `.vibetty-version` 供下次比对。
const VERSION: &str = env!("CARGO_PKG_VERSION");

const SKILL_DIR_NAME: &str = "run-vibetty";
const SKILL_FILE_NAME: &str = "SKILL.md";
const VERSION_FILE_NAME: &str = ".vibetty-version";

#[derive(Clone, Copy)]
enum Agent {
    Claude,
    Codex,
}

impl Agent {
    /// 该 agent 用户级配置根目录(`~/.claude` 或 `~/.agents`)。
    fn root(self) -> &'static str {
        match self {
            Agent::Claude => ".claude",
            Agent::Codex => ".agents",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
        }
    }

    /// 解析该 agent 的 skill 目录(`~/.<root>/skills/run-vibetty`)。home_dir 为 None 时报错。
    fn skill_dir(self) -> anyhow::Result<PathBuf> {
        let home = dirs::home_dir().ok_or_else(|| anyhow!("cannot determine home directory"))?;
        Ok(home.join(self.root()).join("skills").join(SKILL_DIR_NAME))
    }
}

pub fn run_skill(action: SkillAction) -> anyhow::Result<()> {
    match action {
        SkillAction::Install { claude, codex } => {
            for a in resolve_targets(claude, codex)? {
                install_one(a)?;
            }
        }
        SkillAction::Uninstall { claude, codex } => {
            for a in resolve_targets(claude, codex)? {
                uninstall_one(a)?;
            }
        }
    }
    Ok(())
}

/// 把两个 bool 标志展开成目标 agent 列表;两者皆 false 时报错。
fn resolve_targets(claude: bool, codex: bool) -> anyhow::Result<Vec<Agent>> {
    let mut v = Vec::new();
    if claude {
        v.push(Agent::Claude);
    }
    if codex {
        v.push(Agent::Codex);
    }
    if v.is_empty() {
        bail!("specify at least one of --claude or --codex");
    }
    Ok(v)
}

/// 写入 SKILL.md(版本感知、幂等):同版本且已装 → 跳过;否则覆盖并记录版本。
fn install_one(agent: Agent) -> anyhow::Result<()> {
    let dir = agent.skill_dir()?;
    let skill_path = dir.join(SKILL_FILE_NAME);
    let version_path = dir.join(VERSION_FILE_NAME);

    // 同版本且 SKILL.md 在 → 跳过,不重写(避免无谓覆盖 / mtime 抖动)。
    let already_current = skill_path.exists()
        && std::fs::read_to_string(&version_path)
            .ok()
            .is_some_and(|s| s.trim() == VERSION);
    if already_current {
        println!(
            "[{}] already installed (v{}): {}",
            agent.label(),
            VERSION,
            skill_path.display()
        );
        return Ok(());
    }

    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating skill dir {}", dir.display()))?;
    std::fs::write(&skill_path, SKILL_MD.as_bytes())
        .with_context(|| format!("writing {}", skill_path.display()))?;
    std::fs::write(&version_path, VERSION.as_bytes())
        .with_context(|| format!("writing {}", version_path.display()))?;

    println!(
        "[{}] installed v{}: {}",
        agent.label(),
        VERSION,
        skill_path.display()
    );
    Ok(())
}

/// 移除 SKILL.md + `.vibetty-version`;仅当目录随后变空才删目录(绝不递归删)。
fn uninstall_one(agent: Agent) -> anyhow::Result<()> {
    let dir = agent.skill_dir()?;
    let skill_path = dir.join(SKILL_FILE_NAME);
    let version_path = dir.join(VERSION_FILE_NAME);

    if !skill_path.exists() && !version_path.exists() {
        println!("[{}] not installed: {}", agent.label(), dir.display());
        return Ok(());
    }

    if skill_path.exists() {
        std::fs::remove_file(&skill_path)
            .with_context(|| format!("removing {}", skill_path.display()))?;
    }
    if version_path.exists() {
        std::fs::remove_file(&version_path)
            .with_context(|| format!("removing {}", version_path.display()))?;
    }
    println!("[{}] removed: {}", agent.label(), dir.display());

    // 目录现在为空才删(用户可能放了别的文件);只用 remove_dir,绝不 remove_dir_all。
    let empty = std::fs::read_dir(&dir)
        .with_context(|| format!("reading dir {}", dir.display()))?
        .next()
        .is_none();
    if empty {
        let _ = std::fs::remove_dir(&dir);
        println!("[{}] removed empty dir: {}", agent.label(), dir.display());
    }
    Ok(())
}
