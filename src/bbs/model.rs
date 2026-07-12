use serde::Deserialize;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MissionKind {
    Sign,
    Read,
    Like,
    Share,
    Other(i64),
}

impl From<i64> for MissionKind {
    fn from(value: i64) -> Self {
        match value {
            58 => Self::Sign,
            59 => Self::Read,
            60 => Self::Like,
            61 => Self::Share,
            value => Self::Other(value),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MissionProgress {
    pub kind: MissionKind,
    pub award_received: bool,
    pub happened_times: u32,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CoinSummary {
    pub can_get_points: u32,
    pub already_received_points: u32,
    pub total_points: u32,
    pub missions: Vec<MissionProgress>,
}

impl CoinSummary {
    pub fn mission(&self, kind: MissionKind) -> Option<&MissionProgress> {
        self.missions.iter().find(|mission| mission.kind == kind)
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct PostRef {
    pub post_id: String,
    pub subject: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct MissionData {
    #[serde(default)]
    pub can_get_points: u32,
    #[serde(default)]
    pub already_received_points: u32,
    #[serde(default)]
    pub total_points: u32,
    #[serde(default)]
    pub states: Vec<MissionState>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct MissionState {
    pub mission_id: i64,
    #[serde(default)]
    pub is_get_award: bool,
    #[serde(default)]
    pub happened_times: u32,
}

impl From<MissionData> for CoinSummary {
    fn from(value: MissionData) -> Self {
        Self {
            can_get_points: value.can_get_points,
            already_received_points: value.already_received_points,
            total_points: value.total_points,
            missions: value
                .states
                .into_iter()
                .map(|state| MissionProgress {
                    kind: state.mission_id.into(),
                    award_received: state.is_get_award,
                    happened_times: state.happened_times,
                })
                .collect(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct PostListData {
    #[serde(default)]
    pub list: Vec<PostEntry>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PostEntry {
    pub post: PostData,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PostData {
    pub post_id: String,
    #[serde(default)]
    pub subject: String,
}

impl From<PostData> for PostRef {
    fn from(value: PostData) -> Self {
        Self {
            post_id: value.post_id,
            subject: value.subject,
        }
    }
}
