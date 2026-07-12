mod china;
mod games;
mod hoyolab;
mod response;

pub use china::{CaptchaHeaders, CheckinError, ChinaCheckinClient};
pub use games::{ChinaGame, GameSpec, HoyolabGame, HoyolabGameSpec};
pub use hoyolab::{HoyolabCheckinClient, HoyolabCheckinError};
pub use response::{CheckinState, GameRole, Reward, RoleState, SignState};
