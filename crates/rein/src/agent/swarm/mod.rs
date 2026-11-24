// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Collaborative multi-agent swarm module exports

mod board;
mod tool;

pub use board::{
    SwarmAddRequest, SwarmBoardPost, SwarmManager, SwarmMemberState, validate_worker_name,
};
pub use tool::SwarmTool;
