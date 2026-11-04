//! 与平台无关的部分：状态与形状、设置、主题、剪贴板历史、音量计算、格式化。
//! 单独放在库里，方便在任何系统上跑单元测试。

pub mod clip;
pub mod color;
pub mod edit;
pub mod level;
pub mod metrics;
pub mod playback;
pub mod settings;
pub mod shelf;
pub mod state;
pub mod text;
pub mod todo;
pub mod translate;
