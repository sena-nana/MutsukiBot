//! Light link-card layouts for [`CARD_RENDER`].

use html_escape::encode_text;
use mutsuki_protocol_image::{CardLayout, CardRenderRequest, Rgba};
use mutsuki_runtime_contracts::ResourceRef;

pub const COVER_SRC: &str = "mutsuki-card-cover";
pub const CARD_WIDTH: u32 = 720;

const INK: &str = "#111318";
const INK_2: &str = "#5e6570";
const INK_3: &str = "#8b919c";
const ACCENT: &str = "#5e6ad2";
const ROSE: &str = "#c45c4a";
const PAPER: &str = "#ffffff";
const CANVAS: &str = "#eceef2";
const GLASS: &str = "background:rgba(255,255,255,0.78);border:1px solid rgba(255,255,255,0.7);backdrop-filter:blur(18px);";

pub struct CardScene {
    pub html: String,
    pub width: u32,
    pub height: u32,
}

#[must_use]
pub fn card_cover(request: &CardRenderRequest) -> Option<ResourceRef> {
    request.cover.clone()
}

#[must_use]
pub fn compose_card(request: &CardRenderRequest, family: &str) -> CardScene {
    let family = encode_text(family);
    match request.layout {
        CardLayout::Media => media_card(request, &family),
        CardLayout::Hero => hero_card(request, &family),
        CardLayout::Row => row_card(request, &family),
        CardLayout::Feed => feed_card(request, &family),
        CardLayout::Profile => profile_card(request, &family),
        CardLayout::Art => art_card(request, &family),
    }
}

fn media_card(request: &CardRenderRequest, family: &str) -> CardScene {
    scene(
        family,
        644,
        &format!(
            r#"<div style="display:flex;flex-direction:column;width:100%;height:100%;">
  {}
  {}
</div>"#,
            media_block(request, 405),
            info_body(request, 31, 2),
        ),
    )
}

fn hero_card(request: &CardRenderRequest, family: &str) -> CardScene {
    scene(
        family,
        450,
        &format!(
            r#"{}
  <div style="position:absolute;top:12px;left:12px;display:flex;align-items:center;height:44px;padding:0 16px;border-radius:999px;{GLASS}">
    <div style="width:14px;height:14px;border-radius:50%;background:{ROSE};"></div>
    <div style="margin-left:10px;font-size:22px;font-weight:500;color:{INK};">直播</div>
  </div>
  {}"#,
            cover_fill(request),
            dock(request),
        ),
    )
}

fn row_card(request: &CardRenderRequest, family: &str) -> CardScene {
    let (label, dot): (&str, &str) = if request.live {
        ("直播", ROSE)
    } else {
        (&request.brand, ACCENT)
    };
    let kicker = if request.kicker.trim() == label.trim() {
        String::new()
    } else {
        kicker_text(&request.kicker)
    };
    scene(
        family,
        176,
        &format!(
            r#"<div style="display:flex;width:100%;height:100%;">
  <div style="position:relative;width:176px;height:176px;flex:none;overflow:hidden;background:{CANVAS};">{}</div>
  <div style="display:flex;flex-direction:column;justify-content:center;gap:8px;min-width:0;flex:1;padding:20px 24px;">
    <div style="display:flex;align-items:center;gap:16px;">{}{kicker}</div>
    {}
    {}
  </div>
</div>"#,
            cover_fit(request),
            pill(label, dot),
            title_block(&request.title, 28, 1),
            meta_block(&request.description),
        ),
    )
}

fn feed_card(request: &CardRenderRequest, family: &str) -> CardScene {
    let mosaic = if request.cover.is_some() {
        format!(
            r#"<div style="flex:none;overflow:hidden;border-radius:22px;height:371px;background:{CANVAS};">
  <img src="{COVER_SRC}" style="width:100%;height:100%;object-fit:cover;" />
</div>"#
        )
    } else {
        String::new()
    };
    let mosaic_h = u32::from(request.cover.is_some()) * 371;
    let meta_h = u32::from(!request.description.trim().is_empty()) * 34;
    let sections = 2 + u32::from(mosaic_h > 0) + u32::from(meta_h > 0);
    scene(
        family,
        54 + 44 + 126 + mosaic_h + meta_h + sections.saturating_sub(1) * 22,
        &format!(
            r#"<div style="display:flex;flex-direction:column;gap:22px;padding:28px 30px 26px;box-sizing:border-box;width:100%;height:100%;">
  <div style="display:flex;align-items:center;gap:16px;">{}{}</div>
  {}
  {mosaic}
  {}
</div>"#,
            brand_pill(&request.brand),
            kicker_text(&request.kicker),
            text_block(&request.title, 27, 3),
            meta_block(&request.description),
        ),
    )
}

fn profile_card(request: &CardRenderRequest, family: &str) -> CardScene {
    let role_h = u32::from(!request.kicker.trim().is_empty()) * 34;
    let banner = if request.cover.is_some() {
        format!(
            r#"<img src="{COVER_SRC}" style="width:100%;height:100%;object-fit:cover;filter:blur(16px);transform:scale(1.12);" />
      <div style="position:absolute;inset:0;background:linear-gradient(to bottom,rgba(255,255,255,0.08),rgba(255,255,255,0.42));"></div>"#
        )
    } else {
        wash(request)
    };
    let avatar = if request.cover.is_some() {
        format!(r#"<img src="{COVER_SRC}" style="width:100%;height:100%;object-fit:cover;" />"#)
    } else {
        monogram(&request.title)
    };
    let role = if request.kicker.trim().is_empty() {
        String::new()
    } else {
        format!(
            r#"<div style="margin-top:8px;font-size:25px;color:{INK_2};">{}</div>"#,
            encode_text(&request.kicker)
        )
    };
    let bio = if request.description.trim().is_empty() {
        String::new()
    } else {
        format!(
            r#"<div style="margin-top:16px;max-width:100%;font-size:25px;line-height:1.5;color:{INK_2};line-clamp:2;overflow:hidden;">{}</div>"#,
            encode_text(&request.description)
        )
    };
    scene(
        family,
        184 + 64 + 20 + 40 + role_h + 16 + 76 + 32,
        &format!(
            r#"<div style="display:flex;flex-direction:column;width:100%;height:100%;">
  <div style="position:relative;height:184px;overflow:hidden;">{banner}</div>
  <div style="display:flex;flex-direction:column;align-items:center;padding:0 32px 32px;text-align:center;">
    <div style="width:128px;height:128px;margin-top:-64px;overflow:hidden;border:6px solid {PAPER};border-radius:50%;background:{CANVAS};">{avatar}</div>
    <div style="margin-top:20px;font-size:32px;font-weight:600;letter-spacing:-0.022em;color:{INK};">{}</div>
    {role}
    {bio}
  </div>
</div>"#,
            encode_text(&request.title),
        ),
    )
}

fn art_card(request: &CardRenderRequest, family: &str) -> CardScene {
    scene(
        family,
        960,
        &format!(
            r#"{}
  <div style="position:absolute;left:24px;right:24px;bottom:24px;display:flex;flex-direction:column;gap:6px;padding:22px 24px;border-radius:24px;{GLASS}">
    {}
    {}
    {}
  </div>"#,
            cover_fill(request),
            brand_pill(&request.brand),
            title_block(&request.title, 30, 1),
            meta_block(&request.description),
        ),
    )
}

fn scene(family: &str, height: u32, inner: &str) -> CardScene {
    CardScene {
        html: format!(
            r#"<div style="width:{CARD_WIDTH}px;height:{height}px;position:relative;overflow:hidden;box-sizing:border-box;background:{PAPER};border:1px solid rgba(17,19,24,0.08);font-family:'{family}',sans-serif;color:{INK};">
  {inner}
</div>"#
        ),
        width: CARD_WIDTH,
        height,
    }
}

fn media_block(request: &CardRenderRequest, height: u32) -> String {
    let inner = if request.cover.is_some() {
        format!(r#"<img src="{COVER_SRC}" style="width:100%;height:100%;object-fit:cover;" />"#)
    } else {
        format!(
            r#"{}<div style="position:absolute;inset:0;display:flex;align-items:center;justify-content:center;">{}</div>"#,
            wash(request),
            play_glyph()
        )
    };
    format!(
        r#"<div style="position:relative;width:100%;height:{height}px;flex:none;overflow:hidden;background:{CANVAS};">{inner}</div>"#
    )
}

fn info_body(request: &CardRenderRequest, title_size: u32, title_lines: u8) -> String {
    format!(
        r#"<div style="display:flex;flex-direction:column;gap:16px;padding:26px 30px 30px;box-sizing:border-box;">
  <div style="display:flex;align-items:center;gap:16px;">{}{}</div>
  {}
  {}
</div>"#,
        brand_pill(&request.brand),
        kicker_text(&request.kicker),
        title_block(&request.title, title_size, title_lines),
        meta_block(&request.description),
    )
}

fn cover_fill(request: &CardRenderRequest) -> String {
    if request.cover.is_some() {
        format!(
            r#"<img src="{COVER_SRC}" style="position:absolute;inset:0;width:100%;height:100%;object-fit:cover;" />"#
        )
    } else {
        wash(request)
    }
}

fn cover_fit(request: &CardRenderRequest) -> String {
    if request.cover.is_some() {
        format!(r#"<img src="{COVER_SRC}" style="width:100%;height:100%;object-fit:cover;" />"#)
    } else {
        wash(request)
    }
}

fn dock(request: &CardRenderRequest) -> String {
    format!(
        r#"<div style="position:absolute;left:24px;right:24px;bottom:24px;display:flex;flex-direction:column;gap:8px;padding:22px 24px;border-radius:24px;{GLASS}">
  {}
  {}
</div>"#,
        title_block(&request.title, 30, 2),
        meta_block(&request.description)
    )
}

fn brand_pill(brand: &str) -> String {
    pill(brand, ACCENT)
}

fn pill(label: &str, dot: &str) -> String {
    if label.trim().is_empty() {
        return String::new();
    }
    format!(
        r#"<div style="display:flex;align-items:center;gap:12px;height:44px;padding:0 16px 0 14px;border-radius:999px;background:rgba(17,19,24,0.045);">
  <div style="width:12px;height:12px;border-radius:50%;background:{dot};"></div>
  <div style="font-size:22px;font-weight:500;color:{INK_2};">{}</div>
</div>"#,
        encode_text(label)
    )
}

fn kicker_text(kicker: &str) -> String {
    if kicker.trim().is_empty() {
        String::new()
    } else {
        format!(
            r#"<div style="font-size:23px;color:{INK_3};">{}</div>"#,
            encode_text(kicker)
        )
    }
}

fn title_block(title: &str, size: u32, lines: u8) -> String {
    format!(
        r#"<div style="font-size:{size}px;font-weight:600;letter-spacing:-0.022em;line-height:1.32;color:{INK};line-clamp:{lines};overflow:hidden;">{}</div>"#,
        encode_text(title)
    )
}

fn text_block(text: &str, size: u32, lines: u8) -> String {
    format!(
        r#"<div style="font-size:{size}px;line-height:1.55;color:#2a2e36;line-clamp:{lines};overflow:hidden;">{}</div>"#,
        encode_text(text)
    )
}

fn meta_block(meta: &str) -> String {
    if meta.trim().is_empty() {
        String::new()
    } else {
        format!(
            r#"<div style="font-size:24px;line-height:1.4;color:{INK_3};line-clamp:1;overflow:hidden;">{}</div>"#,
            encode_text(meta)
        )
    }
}

fn wash(request: &CardRenderRequest) -> String {
    format!(
        r#"<div style="position:absolute;inset:0;background:radial-gradient(120% 80% at 10% 0%,{},transparent 52%),radial-gradient(90% 70% at 90% 80%,{},transparent 46%),linear-gradient(160deg,#eef0f6 0%,#e2e5ee 52%,#d5dae6 100%);"></div>"#,
        css_rgba_alpha(request.fallback_gradient.start, 0.22),
        css_rgba_alpha(request.fallback_gradient.end, 0.18)
    )
}

fn play_glyph() -> String {
    format!(
        r#"<div style="display:flex;align-items:center;justify-content:center;width:88px;height:88px;border:1px solid rgba(255,255,255,0.7);border-radius:28px;background:rgba(255,255,255,0.58);">
  <div style="width:0;height:0;margin-left:6px;border-top:16px solid transparent;border-bottom:16px solid transparent;border-left:26px solid {INK};"></div>
</div>"#
    )
}

fn monogram(name: &str) -> String {
    let mark = name
        .chars()
        .find(|ch| !ch.is_whitespace())
        .map(|ch| ch.to_string())
        .unwrap_or_else(|| "·".into());
    format!(
        r#"<div style="display:flex;align-items:center;justify-content:center;width:100%;height:100%;background:linear-gradient(160deg,#eef0f6,#d5dae6);font-size:40px;font-weight:600;color:{INK_2};">{}</div>"#,
        encode_text(&mark)
    )
}

fn css_rgba_alpha(color: Rgba, alpha: f32) -> String {
    format!(
        "rgba({},{},{},{:.3})",
        color.red, color.green, color.blue, alpha
    )
}

#[cfg(test)]
mod tests {
    use mutsuki_runtime_contracts::{
        ResourceAccess, ResourceId, ResourceLifetime, ResourceSealState, ResourceSemantic,
    };

    use super::*;

    #[test]
    fn profile_reuses_cover_for_banner_and_avatar() {
        let card = CardRenderRequest {
            brand: "米画师".into(),
            title: "林栖".into(),
            description: "接角色设定与封面。".into(),
            url: "https://www.mihuashi.com/profiles/1".into(),
            cover: Some(sample_ref()),
            layout: CardLayout::Profile,
            kicker: "画师".into(),
            ..CardRenderRequest::default()
        };
        let scene = compose_card(&card, "Noto Sans SC");
        assert_eq!(scene.html.matches(COVER_SRC).count(), 2);
        assert!(scene.html.contains("filter:blur"));
        assert_eq!(card_cover(&card).unwrap().ref_id.as_str(), "cover");
    }

    #[test]
    fn row_card_deduplicates_pill_label_and_kicker() {
        let live = CardRenderRequest {
            brand: "哔哩哔哩".into(),
            title: "【直播】深夜联机一起打游戏！".into(),
            description: "直播状态更新".into(),
            url: "https://live.bilibili.com/1".into(),
            layout: CardLayout::Row,
            kicker: "直播".into(),
            live: true,
            ..CardRenderRequest::default()
        };
        let scene = compose_card(&live, "Noto Sans SC");
        assert_eq!(scene.html.matches(">直播<").count(), 1);

        let poll = CardRenderRequest {
            live: false,
            kicker: "投稿".into(),
            ..live
        };
        let scene = compose_card(&poll, "Noto Sans SC");
        assert!(scene.html.contains(">哔哩哔哩<"));
        assert!(scene.html.contains(">投稿<"));
    }

    fn sample_ref() -> ResourceRef {
        ResourceRef {
            ref_id: "cover".into(),
            resource_id: ResourceId {
                kind_id: "blob".into(),
                slot_id: "cover".into(),
                generation: 1,
                version: 1,
            },
            semantic: ResourceSemantic::FrozenValue,
            provider_id: "memory".into(),
            resource_kind: "blob".into(),
            schema: "image/jpeg".into(),
            version: 1,
            generation: 1,
            access: ResourceAccess::ProviderRpc {
                provider_id: "memory".into(),
                method: "memory".into(),
            },
            size_hint: Some(32),
            content_hash: None,
            lifetime: ResourceLifetime::Persistent,
            lease: None,
            seal_state: ResourceSealState::Sealed,
        }
    }
}
