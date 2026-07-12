use std::{fmt, str::FromStr};

const TAKUMI_BASE: &str = "https://api-takumi.mihoyo.com";
const ZZZ_BASE: &str = "https://act-nap-api.mihoyo.com";

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum HoyolabGame {
    Genshin,
    StarRail,
    Honkai3rd,
    TearsOfThemis,
    ZenlessZoneZero,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HoyolabGameSpec {
    pub game: HoyolabGame,
    pub config_name: &'static str,
    pub display_name: &'static str,
    pub act_id: &'static str,
    pub event_base: &'static str,
    pub sign_game: Option<&'static str>,
}

impl HoyolabGame {
    pub const ALL: [Self; 5] = [
        Self::Genshin,
        Self::StarRail,
        Self::Honkai3rd,
        Self::TearsOfThemis,
        Self::ZenlessZoneZero,
    ];

    pub const fn spec(self) -> HoyolabGameSpec {
        match self {
            Self::Genshin => HoyolabGameSpec {
                game: self,
                config_name: "genshin",
                display_name: "原神",
                act_id: "e202102251931481",
                event_base: "https://sg-hk4e-api.hoyolab.com/event/sol",
                sign_game: None,
            },
            Self::StarRail => HoyolabGameSpec {
                game: self,
                config_name: "star_rail",
                display_name: "崩坏：星穹铁道",
                act_id: "e202303301540311",
                event_base: "https://sg-public-api.hoyolab.com/event/luna/os",
                sign_game: None,
            },
            Self::Honkai3rd => HoyolabGameSpec {
                game: self,
                config_name: "honkai3rd",
                display_name: "崩坏3",
                act_id: "e202110291205111",
                event_base: "https://sg-public-api.hoyolab.com/event/mani",
                sign_game: None,
            },
            Self::TearsOfThemis => HoyolabGameSpec {
                game: self,
                config_name: "tears_of_themis",
                display_name: "未定事件簿",
                act_id: "e202202281857121",
                event_base: "https://sg-public-api.hoyolab.com/event/luna/os",
                sign_game: None,
            },
            Self::ZenlessZoneZero => HoyolabGameSpec {
                game: self,
                config_name: "zenless_zone_zero",
                display_name: "绝区零",
                act_id: "e202406031448091",
                event_base: "https://sg-act-nap-api.hoyolab.com/event/luna/zzz/os",
                sign_game: Some("zzz"),
            },
        }
    }
}

impl fmt::Display for HoyolabGame {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.spec().config_name)
    }
}

impl FromStr for HoyolabGame {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "genshin" => Ok(Self::Genshin),
            "star_rail" | "honkai_sr" | "honkaisr" => Ok(Self::StarRail),
            "honkai3rd" | "honkai3" => Ok(Self::Honkai3rd),
            "tears_of_themis" => Ok(Self::TearsOfThemis),
            "zenless_zone_zero" | "zzz" => Ok(Self::ZenlessZoneZero),
            _ => Err(format!("不支持的 HoYoLAB 游戏：{value}")),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ChinaGame {
    Honkai2,
    Honkai3rd,
    TearsOfThemis,
    Genshin,
    StarRail,
    ZenlessZoneZero,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GameSpec {
    pub game: ChinaGame,
    pub config_name: &'static str,
    pub display_name: &'static str,
    pub player_name: &'static str,
    pub game_biz: &'static str,
    pub act_id: &'static str,
    pub api_base: &'static str,
    pub rewards_path: &'static str,
    pub info_path: &'static str,
    pub sign_path: &'static str,
    pub origin: &'static str,
    pub referer: &'static str,
    pub sign_game: Option<&'static str>,
}

impl ChinaGame {
    pub const ALL: [Self; 6] = [
        Self::Honkai2,
        Self::Honkai3rd,
        Self::TearsOfThemis,
        Self::Genshin,
        Self::StarRail,
        Self::ZenlessZoneZero,
    ];

    pub const fn spec(self) -> GameSpec {
        match self {
            Self::Honkai2 => common_spec(
                self,
                "honkai2",
                "崩坏学园2",
                "玩家",
                "bh2_cn",
                "e202203291431091",
                "https://webstatic.mihoyo.com",
                "https://webstatic.mihoyo.com/bbs/event/signin/bh2/index.html",
                None,
            ),
            Self::Honkai3rd => common_spec(
                self,
                "honkai3rd",
                "崩坏3",
                "舰长",
                "bh3_cn",
                "e202306201626331",
                "https://webstatic.mihoyo.com",
                "https://webstatic.mihoyo.com/bbs/event/signin/bh3/index.html",
                None,
            ),
            Self::TearsOfThemis => common_spec(
                self,
                "tears_of_themis",
                "未定事件簿",
                "律师",
                "nxx_cn",
                "e202202251749321",
                "https://webstatic.mihoyo.com",
                "https://webstatic.mihoyo.com/bbs/event/signin/nxx/index.html",
                None,
            ),
            Self::Genshin => common_spec(
                self,
                "genshin",
                "原神",
                "旅行者",
                "hk4e_cn",
                "e202311201442471",
                "https://act.mihoyo.com",
                "https://act.mihoyo.com/",
                Some("hk4e"),
            ),
            Self::StarRail => common_spec(
                self,
                "star_rail",
                "崩坏：星穹铁道",
                "开拓者",
                "hkrpg_cn",
                "e202304121516551",
                "https://act.mihoyo.com",
                "https://act.mihoyo.com/",
                None,
            ),
            Self::ZenlessZoneZero => GameSpec {
                game: self,
                config_name: "zenless_zone_zero",
                display_name: "绝区零",
                player_name: "绳匠",
                game_biz: "nap_cn",
                act_id: "e202406242138391",
                api_base: ZZZ_BASE,
                rewards_path: "/event/luna/zzz/home",
                info_path: "/event/luna/zzz/info",
                sign_path: "/event/luna/zzz/sign",
                origin: "https://act.mihoyo.com",
                referer: "https://act.mihoyo.com/",
                sign_game: Some("zzz"),
            },
        }
    }
}

#[allow(clippy::too_many_arguments)]
const fn common_spec(
    game: ChinaGame,
    config_name: &'static str,
    display_name: &'static str,
    player_name: &'static str,
    game_biz: &'static str,
    act_id: &'static str,
    origin: &'static str,
    referer: &'static str,
    sign_game: Option<&'static str>,
) -> GameSpec {
    GameSpec {
        game,
        config_name,
        display_name,
        player_name,
        game_biz,
        act_id,
        api_base: TAKUMI_BASE,
        rewards_path: "/event/luna/home",
        info_path: "/event/luna/info",
        sign_path: "/event/luna/sign",
        origin,
        referer,
        sign_game,
    }
}

impl fmt::Display for ChinaGame {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.spec().config_name)
    }
}

impl FromStr for ChinaGame {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "honkai2" | "bh2_cn" => Ok(Self::Honkai2),
            "honkai3rd" | "honkai3" | "bh3_cn" => Ok(Self::Honkai3rd),
            "tears_of_themis" | "nxx_cn" => Ok(Self::TearsOfThemis),
            "genshin" | "hk4e_cn" => Ok(Self::Genshin),
            "star_rail" | "honkai_sr" | "honkaisr" | "hkrpg_cn" => Ok(Self::StarRail),
            "zenless_zone_zero" | "zzz" | "nap_cn" => Ok(Self::ZenlessZoneZero),
            _ => Err(format!("不支持的国服游戏：{value}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_documented_china_games_have_complete_specs() {
        for game in ChinaGame::ALL {
            let spec = game.spec();
            assert!(!spec.config_name.is_empty());
            assert!(!spec.game_biz.is_empty());
            assert!(!spec.act_id.is_empty());
            assert!(spec.api_base.starts_with("https://"));
            assert!(spec.sign_path.starts_with('/'));
        }
    }

    #[test]
    fn accepts_new_and_legacy_game_names() {
        assert_eq!("star_rail".parse(), Ok(ChinaGame::StarRail));
        assert_eq!("honkai_sr".parse(), Ok(ChinaGame::StarRail));
        assert_eq!("nap_cn".parse(), Ok(ChinaGame::ZenlessZoneZero));
    }

    #[test]
    fn zzz_uses_its_dedicated_endpoints() {
        let spec = ChinaGame::ZenlessZoneZero.spec();
        assert_eq!(spec.api_base, ZZZ_BASE);
        assert_eq!(spec.info_path, "/event/luna/zzz/info");
        assert_eq!(spec.sign_game, Some("zzz"));
    }

    #[test]
    fn all_hoyolab_games_have_independent_specs() {
        for game in HoyolabGame::ALL {
            let spec = game.spec();
            assert!(!spec.config_name.is_empty());
            assert!(!spec.act_id.is_empty());
            assert!(spec.event_base.starts_with("https://"));
            assert!(spec.event_base.contains("hoyolab.com"));
        }
        assert_eq!(
            HoyolabGame::ZenlessZoneZero.spec().sign_game,
            Some("zzz")
        );
    }
}
