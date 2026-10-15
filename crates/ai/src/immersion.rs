//! 沉浸守卫（GEN-060～063）：评论回复破第四墙或用机关文号口吻验真时，整段替换为固定文案。
//! 只作用于评论回复，私信不套用（GEN-032）。纯函数。

use std::sync::LazyLock;

use regex::Regex;

struct Patterns {
    meta: Regex,
    official_tone: Regex,
    scam_topic: Regex,
    scam_voice: Regex,
}

static PATTERNS: LazyLock<Option<Patterns>> = LazyLock::new(|| {
    Some(Patterns {
        // GEN-060
        meta: Regex::new(
            r"(?i)游戏内角色|游戏角色|角色设定|官方设定|设定集|剧情设定|(?:基于|属于|与).{0,12}(?:同人|二创|仿写)|同人(?:创作|作品)|二次创作|仿写(?:宣传)?稿|虚构设定|架空创作|并不存在于现实|视频截取|外部视频|拼接素材|粉丝向|fan\s*fic|与.{0,20}设定不符|维基百科|百科词条",
        )
        .ok()?,
        // GEN-061
        official_tone: Regex::new(
            r"(?:未见|缺少|没有).{0,8}治安局.{0,8}(?:标准|正式)?文号|(?:刑侦)?特勤组.{0,8}(?:正式|官方)?(?:通报|通道|渠道)|建议.{0,12}(?:留意|关注|等待).{0,8}(?:治安局|刑侦特勤组|特勤组).{0,8}(?:正式|后续)?(?:通报|通道)",
        )
        .ok()?,
        // GEN-062
        scam_topic: Regex::new(
            r"诈骗|防诈骗|谨防|代充|兑换码|账号密码|违规账号|陌生链接|代练|外挂|个人信息",
        )
        .ok()?,
        scam_voice: Regex::new(r"提醒|防范|郑重|官方|绳匠|保护").ok()?,
    })
});

/// 防诈骗帖的替换文案（GEN-063）。
pub const SCAM_NOTICE_REPLY: &str = "该提醒内容合理。请勿轻信代充、赠送兑换码、陌生链接及索要账号密码等行为；请通过平台认证渠道核实发布者身份，注意保护个人信息与账号安全。";
/// 其他帖子的替换文案（GEN-063）。
pub const RUMOR_REPLY: &str = "根据现有信息，这条帖所述内容暂时无法在绳网内形成可靠交叉印证。发布渠道与表述细节存在疑点，建议先按未证实信息处理，留意后续是否出现管理员认证或多源佐证。";

/// 帖子是不是防诈骗通告（GEN-062）：标题加正文同时命中主题词与口吻词。
pub fn is_scam_notice(post_text: &str) -> bool {
    PATTERNS
        .as_ref()
        .is_some_and(|p| p.scam_topic.is_match(post_text) && p.scam_voice.is_match(post_text))
}

/// 守卫的结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Keep,
    /// 整段替换；`reason` 记日志（`meta` / `official_tone`）。
    Replace {
        text: &'static str,
        reason: &'static str,
    },
}

/// 检查一条评论回复。`post_text` 是帖子标题加正文。
pub fn check(reply: &str, post_text: &str) -> Verdict {
    let Some(p) = PATTERNS.as_ref() else {
        return Verdict::Keep;
    };
    let scam = is_scam_notice(post_text);
    let replacement = if scam { SCAM_NOTICE_REPLY } else { RUMOR_REPLY };
    if p.meta.is_match(reply) {
        return Verdict::Replace {
            text: replacement,
            reason: "meta",
        };
    }
    // 防诈骗帖本身提到治安局、特勤组是正常的，不拦（GEN-061）。
    if !scam && p.official_tone.is_match(reply) {
        return Verdict::Replace {
            text: RUMOR_REPLY,
            reason: "official_tone",
        };
    }
    Verdict::Keep
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_compile() {
        assert!(PATTERNS.is_some());
    }

    #[test]
    fn in_world_replies_pass() {
        assert_eq!(
            check("这条委托我查过了，暂时没有异常。", "委托"),
            Verdict::Keep
        );
    }

    #[test]
    fn breaking_the_fourth_wall_is_replaced() {
        assert_eq!(
            check("这是基于游戏的同人创作。", "新艾利都传闻"),
            Verdict::Replace {
                text: RUMOR_REPLY,
                reason: "meta"
            }
        );
        assert!(matches!(
            check("看起来是 Fan Fic 吧", "传闻"),
            Verdict::Replace { reason: "meta", .. }
        ));
    }

    #[test]
    fn official_tone_depends_on_the_post() {
        let reply = "目前未见治安局正式文号，建议等待特勤组正式通报。";
        assert_eq!(
            check(reply, "六分街出现怪物"),
            Verdict::Replace {
                text: RUMOR_REPLY,
                reason: "official_tone"
            }
        );
        let scam_post = "郑重提醒：谨防代充诈骗，不要泄露账号密码";
        assert!(is_scam_notice(scam_post));
        assert_eq!(check(reply, scam_post), Verdict::Keep);
        assert_eq!(
            check("这是官方设定里的内容", scam_post),
            Verdict::Replace {
                text: SCAM_NOTICE_REPLY,
                reason: "meta"
            }
        );
    }
}
