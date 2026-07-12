use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentState {
    Working,
    Waiting,
}

impl AgentState {
    pub fn is_waiting(&self) -> bool {
        matches!(self, AgentState::Waiting)
    }
}

pub trait Agent {
    fn state(&self) -> AgentState;
    /// 按当前 title 更新状态,返回状态是否发生变化。
    fn update_by_title(&mut self, title: &str) -> bool;
}

pub struct CodexAgent {
    initial_title: String,
    title_initialized: bool,
    current_state: AgentState,
}

impl Agent for CodexAgent {
    fn state(&self) -> AgentState {
        self.current_state
    }

    fn update_by_title(&mut self, title: &str) -> bool {
        if !self.title_initialized {
            self.initial_title = title.to_string();
            self.title_initialized = true;
        }

        let new_state = if title == self.initial_title
            || title.starts_with("[ . ] Action Required")
            || title.starts_with("[ ! ] Action Required")
        {
            AgentState::Waiting
        } else {
            AgentState::Working
        };

        let changed = self.current_state != new_state;
        self.current_state = new_state;
        changed
    }
}

pub struct ClaudeCodeAgent {
    current_state: AgentState,
}

impl Agent for ClaudeCodeAgent {
    fn state(&self) -> AgentState {
        self.current_state
    }

    fn update_by_title(&mut self, title: &str) -> bool {
        let new_state = if title.starts_with('✳') {
            AgentState::Waiting
        } else {
            AgentState::Working
        };
        let changed = self.current_state != new_state;
        self.current_state = new_state;
        changed
    }
}

pub struct UnSupportedAgent;

impl Agent for UnSupportedAgent {
    fn state(&self) -> AgentState {
        AgentState::Working
    }

    fn update_by_title(&mut self, _title: &str) -> bool {
        false
    }
}

pub enum AgentType {
    Codex(CodexAgent),
    ClaudeCode(ClaudeCodeAgent),
    UnSupported(UnSupportedAgent),
}

impl AgentType {
    pub fn new(agent_type: &str) -> Self {
        if agent_type.ends_with("codex") {
            Self::Codex(CodexAgent {
                initial_title: String::new(),
                title_initialized: false,
                current_state: AgentState::Working,
            })
        } else if agent_type.ends_with("claude") {
            Self::ClaudeCode(ClaudeCodeAgent {
                current_state: AgentState::Working,
            })
        } else {
            log::warn!(
                "Unsupported agent type: {}. Defaulting to UnSupportedAgent.",
                agent_type
            );
            Self::UnSupported(UnSupportedAgent)
        }
    }

    pub fn state(&self) -> AgentState {
        match self {
            Self::Codex(agent) => agent.state(),
            Self::ClaudeCode(agent) => agent.state(),
            Self::UnSupported(agent) => agent.state(),
        }
    }

    /// 按当前 title 更新状态,返回状态是否发生变化。
    pub fn update_by_title(&mut self, title: &str) -> bool {
        match self {
            Self::Codex(agent) => agent.update_by_title(title),
            Self::ClaudeCode(agent) => agent.update_by_title(title),
            Self::UnSupported(agent) => agent.update_by_title(title),
        }
    }
}
