mod client;
mod model;

pub use client::{BbsClient, BbsEndpoints, BbsError, ForumSignRequest, Verification};
pub use model::{CoinSummary, MissionKind, MissionProgress, PostRef};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ForumSpec {
    pub id: u8,
    pub gids: &'static str,
    pub forum_id: &'static str,
    pub name: &'static str,
}

/// 与原 Python 项目的 `mihoyobbs_List` 保持一致。
pub const SUPPORTED_FORUMS: [ForumSpec; 9] = [
    ForumSpec {
        id: 1,
        gids: "1",
        forum_id: "1",
        name: "崩坏3",
    },
    ForumSpec {
        id: 2,
        gids: "2",
        forum_id: "26",
        name: "原神",
    },
    ForumSpec {
        id: 3,
        gids: "3",
        forum_id: "30",
        name: "崩坏学园2",
    },
    ForumSpec {
        id: 4,
        gids: "4",
        forum_id: "37",
        name: "未定事件簿",
    },
    ForumSpec {
        id: 5,
        gids: "5",
        forum_id: "34",
        name: "大别野",
    },
    ForumSpec {
        id: 6,
        gids: "6",
        forum_id: "52",
        name: "崩坏：星穹铁道",
    },
    ForumSpec {
        id: 8,
        gids: "8",
        forum_id: "57",
        name: "绝区零",
    },
    ForumSpec {
        id: 9,
        gids: "9",
        forum_id: "948",
        name: "崩坏：因缘精灵",
    },
    ForumSpec {
        id: 10,
        gids: "10",
        forum_id: "950",
        name: "星布谷地",
    },
];

pub fn forum_by_id(id: u8) -> Option<&'static ForumSpec> {
    SUPPORTED_FORUMS.iter().find(|forum| forum.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forum_mapping_matches_the_legacy_python_project() {
        assert_eq!(forum_by_id(5).map(|forum| forum.forum_id), Some("34"));
        assert_eq!(forum_by_id(2).map(|forum| forum.forum_id), Some("26"));
        assert_eq!(forum_by_id(10).map(|forum| forum.name), Some("星布谷地"));
        assert!(forum_by_id(7).is_none());
    }
}
