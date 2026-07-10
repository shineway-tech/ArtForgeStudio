//! ArtAIT 核心数据模型。
//!
//! 此 crate 是依赖图的最底层：零异步、零 IO、不依赖任何上层 crate。
//! 所有跨 crate 共享的数据结构、枚举、错误类型都在这里。

pub mod app_config;
pub mod asset;
pub mod character;
pub mod cinematography;
pub mod director;
pub mod feature;
pub mod paths;
pub mod project;
pub mod prompt;
pub mod provider;
pub mod scene;
pub mod script;
pub mod seedance;
pub mod task;
pub mod theme;

pub use app_config::*;
pub use asset::*;
pub use character::*;
pub use director::*;
pub use feature::*;
pub use paths::*;
pub use prompt::*;
pub use provider::*;
pub use task::*;
pub use theme::*;
