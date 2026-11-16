// SPDX-License-Identifier: AGPL-3.0-or-later
//! Every colour Inlet draws with, in one place. Values follow the Fluxer parity spec
//! (Fluxer's default dark theme, computed to sRGB); views never hard-code colours.
//!
//! There are three themes ([`Theme`]): dark, coal and light. A colour function answers
//! for the theme being drawn ([`current`]), so a theme change shows from the next frame
//! on, without a restart. Light follows the spec's values (`hsl(220,10%,...)`); coal is
//! the dark theme on the spec's 1 to 12 % lightness scale (the shell and the chat share
//! the one background, `hsl(258,10%,1%)`). Roles the spec gives no value for are
//! derived: coal takes the dark value's place on its shorter scale, light a value in
//! the same family at the matching lightness.
//!
//! Brand, link and mention colours can be overridden per instance (an instance's
//! `branding.theme_color`), the way Fluxer's own client applies instance branding.

use std::cell::Cell;
use std::sync::atomic::{AtomicU32, Ordering};

use gpui::{rgb, rgba, Hsla, Rgba, WindowAppearance};

fn h(c: Rgba) -> Hsla {
    c.into()
}

// ---- Which theme ---------------------------------------------------------------

/// A colour theme that is drawn. Fluxer has four: dark, coal, light and "follow the
/// system", which is one of the first three at any moment ([`Pref`] is the choice).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Theme {
    /// Fluxer's default dark theme.
    #[default]
    Dark,
    /// Nearly black: the shell and the chat share one background.
    Coal,
    /// The light theme.
    Light,
}

/// What the person chose in the settings: a theme, or to follow the operating system.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Pref {
    #[default]
    Dark,
    Coal,
    Light,
    System,
}

impl Pref {
    /// Every choice, in the order the settings show them.
    pub const ALL: [Pref; 4] = [Pref::Dark, Pref::Coal, Pref::Light, Pref::System];

    /// The server's name for it (`theme` in the user settings).
    pub fn server_name(self) -> &'static str {
        match self {
            Pref::Dark => "dark",
            Pref::Coal => "coal",
            Pref::Light => "light",
            Pref::System => "system",
        }
    }

    /// From the server's `theme`. `dark_legacy` (Fluxer's older dark palette) is drawn
    /// as dark; an empty or unknown name gives `None`.
    pub fn from_server(name: &str) -> Option<Pref> {
        match name {
            "dark" | "dark_legacy" => Some(Pref::Dark),
            "coal" => Some(Pref::Coal),
            "light" => Some(Pref::Light),
            "system" => Some(Pref::System),
            _ => None,
        }
    }

    /// The theme to draw, given what the operating system prefers.
    pub fn resolve(self, system_dark: bool) -> Theme {
        match self {
            Pref::Dark => Theme::Dark,
            Pref::Coal => Theme::Coal,
            Pref::Light => Theme::Light,
            Pref::System if system_dark => Theme::Dark,
            Pref::System => Theme::Light,
        }
    }
}

/// Whether a window appearance is a dark one.
pub fn appearance_is_dark(appearance: WindowAppearance) -> bool {
    matches!(
        appearance,
        WindowAppearance::Dark | WindowAppearance::VibrantDark
    )
}

// The theme belongs to the UI thread, which draws and changes it. Keeping it per thread
// also keeps tests that draw with another theme from changing each other's colours.
thread_local! {
    static THEME: Cell<Theme> = const { Cell::new(Theme::Dark) };
    static SYSTEM_DARK: Cell<bool> = const { Cell::new(true) };
}

/// The theme being drawn.
pub fn current() -> Theme {
    THEME.with(Cell::get)
}

/// Draws with `theme` from the next frame on.
pub fn set_theme(theme: Theme) {
    THEME.with(|t| t.set(theme));
}

/// Whether the operating system prefers a dark appearance (dark until it says otherwise).
pub fn system_is_dark() -> bool {
    SYSTEM_DARK.with(Cell::get)
}

/// Notes what the operating system prefers, for the choice "system".
pub fn set_system_dark(dark: bool) {
    SYSTEM_DARK.with(|t| t.set(dark));
}

/// Draws with what `pref` stands for now.
pub fn apply(pref: Pref) {
    set_theme(pref.resolve(system_is_dark()));
}

/// A colour of the current theme (`0xRRGGBB`).
fn pick(dark: u32, coal: u32, light: u32) -> Hsla {
    h(rgb(match current() {
        Theme::Dark => dark,
        Theme::Coal => coal,
        Theme::Light => light,
    }))
}

/// A colour with opacity of the current theme (`0xRRGGBBAA`).
fn pick_a(dark: u32, coal: u32, light: u32) -> Hsla {
    h(rgba(match current() {
        Theme::Dark => dark,
        Theme::Coal => coal,
        Theme::Light => light,
    }))
}

/// The colours of a small picture of a theme: the settings show every theme as it looks,
/// whichever one is drawn.
#[derive(Clone, Copy, Debug)]
pub struct Preview {
    pub shell: Hsla,
    pub chat: Hsla,
    pub text: Hsla,
    pub text_muted: Hsla,
    pub brand: Hsla,
}

/// The preview colours of `theme`.
pub fn preview(theme: Theme) -> Preview {
    let before = current();
    set_theme(theme);
    let p = Preview {
        shell: bg_app(),
        chat: bg_chat(),
        text: text(),
        text_muted: text_channel(),
        brand: brand(),
    };
    set_theme(before);
    p
}

// ---- Instance accent -----------------------------------------------------------

const NONE: u32 = u32::MAX;
static BRAND: AtomicU32 = AtomicU32::new(NONE);

/// Applies an instance accent colour (`0xRRGGBB`), or restores the defaults with `None`.
/// Links and mentions get a version of the accent that reads on the background: a light
/// one on the dark themes, a dark one on the light theme.
pub fn set_accent(accent: Option<u32>) {
    BRAND.store(accent.map_or(NONE, |c| c & 0xFF_FF_FF), Ordering::Relaxed);
}

/// Parses `#rrggbb` (the instance's `theme_color`).
pub fn parse_hex(s: &str) -> Option<u32> {
    let hex = s.trim().strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    u32::from_str_radix(hex, 16).ok()
}

/// The accent at the given lightness, keeping its hue: readable on the background.
fn with_lightness(c: u32, l: f32) -> u32 {
    let hsla: Hsla = rgb(c).into();
    let tuned = Hsla {
        l,
        s: hsla.s.max(0.6),
        ..hsla
    };
    let r: Rgba = tuned.into();
    let to = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    (to(r.r) << 16) | (to(r.g) << 8) | to(r.b)
}

fn brand_override() -> Option<u32> {
    match BRAND.load(Ordering::Relaxed) {
        NONE => None,
        c => Some(c),
    }
}

// ---- Message display -----------------------------------------------------------

/// Fluxer's default message font size, in logical pixels.
pub const DEFAULT_FONT_PX: f32 = 16.;
/// Fluxer's default space above the first message of a group, in logical pixels.
pub const DEFAULT_GROUP_SPACING: f32 = 16.;

thread_local! {
    static FONT_PX: Cell<f32> = const { Cell::new(DEFAULT_FONT_PX) };
    static GROUP_SPACING: Cell<f32> = const { Cell::new(DEFAULT_GROUP_SPACING) };
}

/// The font size of message text.
pub fn message_font_px() -> f32 {
    FONT_PX.with(Cell::get)
}

/// The line height of message text: Fluxer's 22 px for 16 px text, in proportion.
pub fn message_line_px() -> f32 {
    (message_font_px() * 22. / 16.).round()
}

/// Sets the font size of message text (as Fluxer: 8 to 48 px).
pub fn set_message_font_px(px: f32) {
    let px = if px.is_finite() { px } else { DEFAULT_FONT_PX };
    FONT_PX.with(|c| c.set(px.clamp(8., 48.)));
}

/// The space above the first message of a group (cozy display only).
pub fn group_spacing() -> f32 {
    GROUP_SPACING.with(Cell::get)
}

/// Sets the space above the first message of a group.
pub fn set_group_spacing(px: f32) {
    let px = if px.is_finite() {
        px
    } else {
        DEFAULT_GROUP_SPACING
    };
    GROUP_SPACING.with(|c| c.set(px.clamp(0., 64.)));
}

// ---- Backgrounds -------------------------------------------------------------

/// Shell: rail, channel list, DM list (`--background-secondary`).
pub fn bg_app() -> Hsla {
    pick(0x1a181e, 0x020203, 0xebecef)
}
/// Chat area and channel header (`--background-secondary-lighter`).
pub fn bg_chat() -> Hsla {
    pick(0x1e1d23, 0x020203, 0xf0f1f3)
}
/// Composer box and inputs (`--background-textarea`).
pub fn bg_textarea() -> Hsla {
    pick(0x242229, 0x100f13, 0xf3f4f5)
}
/// Guild icons without an image, icon circles (`--guild-list-foreground`).
pub fn guild_item() -> Hsla {
    pick(0x29262f, 0x131216, 0xdcdee2)
}
/// Floating user panel (`--panel-control-bg`).
pub fn panel_bg() -> Hsla {
    pick(0x201d25, 0x0e0d10, 0xf5f6f7)
}
/// Embed cards (`--background-primary`).
pub fn embed_bg() -> Hsla {
    pick(0x0c0b0e, 0x020203, 0xfbfbfc)
}
/// Code blocks (`--code-block-bg`).
pub fn code_bg() -> Hsla {
    pick(0x211e26, 0x0e0d10, 0xe7e9eb)
}
/// Inline code (`--code-inline-bg`).
pub fn code_inline_bg() -> Hsla {
    pick(0x242129, 0x100f12, 0xe3e5e8)
}
/// Rows and messages under the pointer: white 5 %.
pub fn hover_bg() -> Hsla {
    pick_a(0xffffff0d, 0xffffff0d, 0x17191c0d)
}
/// Selected channel or DM: white 10 %.
pub fn selected_bg() -> Hsla {
    pick_a(0xffffff1a, 0xffffff1a, 0x17191c1a)
}

// ---- Text -------------------------------------------------------------------

/// Message text (`--text-chat`).
pub fn text() -> Hsla {
    pick(0xeeedf0, 0xeeedf0, 0x30343b)
}
/// Names, titles, selected channel (`--text-primary`).
pub fn text_strong() -> Hsla {
    pick(0xf4f4f6, 0xf4f4f6, 0x22252a)
}
/// Unread channel names (`--text-secondary`).
pub fn text_secondary() -> Hsla {
    pick(0xe5e4e9, 0xe5e4e9, 0x454a54)
}
/// Muted: reply line, "(edited)", header icons (`--text-primary-muted`).
pub fn text_muted() -> Hsla {
    pick(0xcecbd4, 0xcecbd4, 0x616876)
}
/// Unselected channel names and icons, category names (`--text-tertiary-muted`).
pub fn text_channel() -> Hsla {
    pick(0x9d97aa, 0x9d97aa, 0x7c8393)
}
/// Topic, date separators, placeholder (`--text-tertiary`).
pub fn text_tertiary() -> Hsla {
    pick(0xb1acbb, 0xb1acbb, 0x6e7686)
}
/// Message timestamps: muted at 60 %.
pub fn timestamp() -> Hsla {
    pick_a(0xcecbd499, 0xcecbd499, 0x61687699)
}
/// Code text (`--text-code`).
pub fn code_text() -> Hsla {
    pick(0xe7e2e8, 0xe7e2e8, 0x32363e)
}

// ---- Brand, links, mentions --------------------------------------------------

/// Brand colour: BOT tag, active guild, default embed edge (`--brand-primary`).
pub fn brand() -> Hsla {
    h(rgb(brand_override().unwrap_or(0x413cdd)))
}
/// Links (`--text-link`).
pub fn link() -> Hsla {
    match (brand_override(), current()) {
        (Some(accent), Theme::Light) => h(rgb(with_lightness(accent, 0.38))),
        (Some(accent), _) => h(rgb(with_lightness(accent, 0.70))),
        (None, _) => pick(0x6ccff9, 0x6ccff9, 0x099ddc),
    }
}
/// Mention pill text (`--markup-mention-text`).
pub fn mention_text() -> Hsla {
    link()
}
/// Mention pill fill: mention colour at 20 %.
pub fn mention_fill() -> Hsla {
    let mut c = link();
    c.a = 0.2;
    c
}
/// Text selection in inputs (`--text-selection`).
pub fn selection() -> Hsla {
    let mut c = link();
    c.a = 0.35;
    c
}
/// The bar of a message that mentions you.
pub fn mention_me_bar() -> Hsla {
    pick(0xeac532, 0xeac532, 0xeac532)
}
/// The background of a message that mentions you.
pub fn mention_me_bg() -> Hsla {
    h(rgba(0xeac5321a))
}
/// The background of a message that was jumped to (`rgb(59 130 246 / 0.1)`).
pub fn jumped_bg() -> Hsla {
    h(rgba(0x3b82f61a))
}
/// The bar of a message that was jumped to (`--brand-primary-light`).
pub fn jumped_bar() -> Hsla {
    pick(0xb0adff, 0xb0adff, 0x413cdd)
}
/// The reply spine: `--text-chat-muted` 82 % mixed with black.
pub fn reply_spine() -> Hsla {
    pick(0xa9a7ae, 0xa9a7ae, 0x89909f)
}
/// Text on the brand colour (`--brand-primary-fill`).
pub fn on_brand() -> Hsla {
    h(rgb(0xffffff))
}

// ---- Lines and states -------------------------------------------------------

/// Header and panel dividers: white about 3.5 %.
pub fn divider() -> Hsla {
    pick_a(0xffffff09, 0xffffff09, 0x17191c14)
}
/// Component borders such as the embed frame (`--background-modifier-accent`).
pub fn accent_border() -> Hsla {
    pick_a(0xc9c5d326, 0xc9c5d326, 0x1f23291f)
}
/// Subtle ring around floating surfaces (composer, user panel).
pub fn ring() -> Hsla {
    pick_a(0xc9c5d30a, 0xc9c5d30a, 0x17191c0f)
}
/// Input focus ring (`--border-color-focus`).
pub fn focus_ring() -> Hsla {
    pick_a(0x6ccff973, 0x6ccff973, 0x099ddc73)
}
/// Blockquote bar (`--interactive-muted`).
pub fn quote_bar() -> Hsla {
    pick(0x7170a1, 0x7170a1, 0xabb0ba)
}
/// Errors, unread counts (`--status-danger`).
pub fn danger() -> Hsla {
    h(rgb(0xe53734))
}
/// Text on the unread count (`white` on `--status-danger`).
pub fn on_danger() -> Hsla {
    h(rgb(0xffffff))
}
/// Date separator line: accent border at 30 %.
pub fn separator_line() -> Hsla {
    pick_a(0xc9c5d30b, 0xc9c5d30b, 0x17191c14)
}

// ---- Status dots (`--status-online`, `-idle`, `-dnd`, `-offline`) -------------

/// Online: `hsl(152,72%,40%)`.
pub fn status_online() -> Hsla {
    pick(0x1daf6b, 0x1daf6b, 0x1fad6b)
}
/// Idle: `hsl(45,93%,50%)`.
pub fn status_idle() -> Hsla {
    pick(0xf6bb09, 0xf6bb09, 0xdaa60b)
}
/// Do not disturb: `hsl(0,84%,60%)`.
pub fn status_dnd() -> Hsla {
    pick(0xef4343, 0xef4343, 0xd92629)
}
/// Offline and invisible: `hsl(218,11%,65%)`.
pub fn status_offline() -> Hsla {
    h(rgb(0x9ca3b0))
}

// ---- Media ------------------------------------------------------------------

/// `--brand-primary-foreground` without an instance accent.
pub const BRAND_FOREGROUND_DEFAULT: u32 = 0x413cdd;
/// The left bar of an embed without a colour (`--brand-primary-foreground`). It follows
/// the instance accent like the brand colour.
pub fn brand_foreground() -> Hsla {
    h(rgb(brand_override().unwrap_or(BRAND_FOREGROUND_DEFAULT)))
}
/// An image's box while it loads or when it cannot be shown.
pub fn media_placeholder() -> Hsla {
    pick(0x2a2830, 0x131216, 0xd8dadf)
}
/// The cover over a spoiler or explicit image.
pub fn media_cover() -> Hsla {
    pick(0x141317, 0x050506, 0xc7cad1)
}
/// Buttons and labels drawn over media (black at 60 %).
pub fn media_overlay() -> Hsla {
    h(rgba(0x00000099))
}
/// The same under the pointer.
pub fn media_overlay_hover() -> Hsla {
    h(rgba(0x000000cc))
}
/// Behind the image viewer (black at 85 %).
pub fn viewer_backdrop() -> Hsla {
    h(rgba(0x000000d9))
}

// ---- Markdown ---------------------------------------------------------------

/// Code block border: 90 % `--border-color`, 10 % `--text-code` (`--code-block-border`).
pub fn code_border() -> Hsla {
    pick_a(0xa19bad47, 0xa19bad47, 0x7a738c47)
}
/// Table borders (`--border-color`, `hsla(258,10%,50%,0.2)`).
pub fn table_border() -> Hsla {
    pick_a(0x7a738c33, 0x7a738c33, 0x7a738c33)
}
/// Table header row (`--bg-table-header`).
pub fn table_header_bg() -> Hsla {
    pick(0x2a2730, 0x141217, 0xe0e2e6)
}
/// Odd table rows (`--bg-table-row-odd`).
pub fn table_row_odd() -> Hsla {
    pick(0x0c0b0e, 0x020203, 0xfbfbfc)
}
/// Even table rows (`--bg-table-row-even`).
pub fn table_row_even() -> Hsla {
    pick(0x1a181e, 0x020203, 0xf0f1f3)
}
/// A hidden spoiler: the text colour at 16 % (`--markup-spoiler-hidden-bg`).
pub fn spoiler_hidden() -> Hsla {
    let mut c = text();
    c.a = 0.16;
    c
}
/// A revealed spoiler (`--background-modifier-accent`).
pub fn spoiler_revealed() -> Hsla {
    accent_border()
}
/// A mention nobody can resolve: muted text on a muted fill.
pub fn mention_neutral_text() -> Hsla {
    text_muted()
}
/// Fill of a mention nobody can resolve.
pub fn mention_neutral_fill() -> Hsla {
    let mut c = text_muted();
    c.a = 0.16;
    c
}
/// `> [!NOTE]` (`--alert-note-color`, `hsl(198,92%,70%)`).
pub fn alert_note() -> Hsla {
    pick(0x6ccff9, 0x6ccff9, 0x0885ba)
}
/// `> [!TIP]` (`hsl(152,72%,45%)`).
pub fn alert_tip() -> Hsla {
    pick(0x20c578, 0x20c578, 0x178c56)
}
/// `> [!IMPORTANT]` (`hsl(278,85%,65%)`).
pub fn alert_important() -> Hsla {
    pick(0xba5af2, 0xba5af2, 0x9125d0)
}
/// `> [!WARNING]` (`hsl(45,93%,55%)`).
pub fn alert_warning() -> Hsla {
    pick(0xf7c222, 0xf7c222, 0xbd7905)
}
/// `> [!CAUTION]` (`hsl(359,75%,60%)`).
pub fn alert_caution() -> Hsla {
    pick(0xe54c4f, 0xe54c4f, 0xd02528)
}

// Code highlighting: the GitHub Dark colours Fluxer's code blocks use.

/// Keywords.
pub fn syntax_keyword() -> Hsla {
    pick(0xff7b72, 0xff7b72, 0xcf222e)
}
/// Strings.
pub fn syntax_string() -> Hsla {
    pick(0xa5d6ff, 0xa5d6ff, 0x0a3069)
}
/// Comments.
pub fn syntax_comment() -> Hsla {
    pick(0x8b949e, 0x8b949e, 0x6e7781)
}
/// Numbers and other constants, attributes.
pub fn syntax_constant() -> Hsla {
    pick(0x79c0ff, 0x79c0ff, 0x0550ae)
}
/// Function names.
pub fn syntax_function() -> Hsla {
    pick(0xd2a8ff, 0xd2a8ff, 0x8250df)
}
/// Type names and parameters.
pub fn syntax_type() -> Hsla {
    pick(0xffa657, 0xffa657, 0x953800)
}
/// Markup tags, inserted lines.
pub fn syntax_tag() -> Hsla {
    pick(0x7ee787, 0x7ee787, 0x116329)
}

// ---- Message actions -----------------------------------------------------------

/// The action bar, menus and popovers over the chat (`--background-tertiary`).
pub fn floating_bg() -> Hsla {
    pick(0x2a2730, 0x141217, 0xf8f8f9)
}
/// The shadow under them (`0 6px 14px rgb(0 0 0 / 0.18)`).
pub fn floating_shadow() -> Hsla {
    h(rgba(0x0000002e))
}
/// An action button under the pointer (`--surface-interactive-hover-bg`).
pub fn floating_hover() -> Hsla {
    selected_bg()
}
/// A reaction bubble (`--background-secondary-alt`).
pub fn reaction_bg() -> Hsla {
    pick(0x29262f, 0x131216, 0xe0e2e6)
}
/// A reaction of the account: brand at 15 % over the chat.
pub fn reaction_me_bg() -> Hsla {
    let mut c = brand();
    c.a = 0.15;
    c
}
/// The edge of a reaction of the account: the brand colour.
pub fn reaction_me_border() -> Hsla {
    brand()
}
/// The count of a reaction of the account (`--brand-primary-light`).
pub fn reaction_me_text() -> Hsla {
    pick(0xb0adff, 0xb0adff, 0x413cdd)
}

// ---- Panels (pins, inbox) ------------------------------------------------------

/// A message card in a panel (`--message-preview-card-background`: white 2.2 %).
pub fn preview_card_bg() -> Hsla {
    pick_a(0xffffff06, 0xffffff06, 0x17191c06)
}
/// Its border (`--message-preview-card-border`: white 7 %).
pub fn preview_card_border() -> Hsla {
    pick_a(0xffffff12, 0xffffff12, 0x17191c12)
}
/// The line under a card's header (`--message-preview-card-divider`: white 5 %).
pub fn preview_card_divider() -> Hsla {
    pick_a(0xffffff0d, 0xffffff0d, 0x17191c0d)
}

// ---- Member list ------------------------------------------------------------

/// The guild owner's crown: `hsl(39,57%,64%)` (`MemberListItem` `.crownIcon`).
pub fn crown() -> Hsla {
    h(rgb(0xd8b36f))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(c: Hsla) -> u32 {
        let r: Rgba = c.into();
        let to = |v: f32| (v * 255.0).round() as u32;
        (to(r.r) << 16) | (to(r.g) << 8) | to(r.b)
    }

    fn alpha(c: Hsla) -> f32 {
        let r: Rgba = c.into();
        r.a
    }

    /// The values the parity spec gives for the light theme (section 1.7, from
    /// `ThemeVariableManifest.ts`), and for coal (`ColorSystemPalette.ts`: the shell and
    /// the chat take the embed background, `hsl(258,10%,1%)`). The dark values stay.
    #[test]
    fn light_and_coal_themes_match_the_spec_values() {
        set_theme(Theme::Light);
        assert_eq!(hex(embed_bg()), 0xfbfbfc, "--background-primary");
        assert_eq!(hex(bg_app()), 0xebecef, "--background-secondary");
        assert_eq!(hex(bg_chat()), 0xf0f1f3, "--background-secondary-lighter");
        assert_eq!(hex(bg_textarea()), 0xf3f4f5, "--background-textarea");
        assert_eq!(hex(guild_item()), 0xdcdee2, "--guild-list-foreground");
        assert_eq!(hex(text_strong()), 0x22252a, "--text-primary");
        assert_eq!(hex(text()), 0x30343b, "--text-chat");
        assert_eq!(hex(text_muted()), 0x616876, "--text-primary-muted");
        assert_eq!(hex(text_tertiary()), 0x6e7686, "--text-tertiary");
        assert_eq!(hex(link()), 0x099ddc, "--text-link");
        assert_eq!(hex(brand()), 0x413cdd, "the brand colour is the same");
        // Hover and selected: hsla(220,10%,10%) at 5 % and 10 %.
        assert_eq!(hex(hover_bg()), 0x17191c);
        assert!((alpha(hover_bg()) - 0.05).abs() < 0.01);
        assert!((alpha(selected_bg()) - 0.10).abs() < 0.01);
        assert_eq!(hex(status_online()), 0x1fad6b, "hsl(152,70%,40%)");
        assert_eq!(hex(status_idle()), 0xdaa60b, "hsl(45,90%,45%)");
        assert_eq!(hex(status_dnd()), 0xd92629, "hsl(359,70%,50%)");

        set_theme(Theme::Coal);
        assert_eq!(hex(bg_app()), 0x020203, "hsl(258,10%,1%)");
        assert_eq!(hex(bg_chat()), 0x020203, "secondary-lighter is the primary");
        assert_eq!(hex(embed_bg()), 0x020203);
        assert!(
            hex(bg_textarea()) > hex(bg_app()),
            "the composer box still stands out from the chat"
        );
        assert_eq!(hex(text()), 0xeeedf0, "coal keeps the dark theme's text");
        assert_eq!(hex(link()), 0x6ccff9);

        set_theme(Theme::Dark);
        assert_eq!(hex(bg_app()), 0x1a181e);
        assert_eq!(hex(bg_chat()), 0x1e1d23);
        assert_eq!(hex(embed_bg()), 0x0c0b0e);
        assert_eq!(hex(text()), 0xeeedf0);
        assert_eq!(hex(link()), 0x6ccff9);
    }

    #[test]
    fn every_colour_follows_the_theme_it_is_drawn_in() {
        // A light theme leaves no dark surface behind: the shell, the chat, the box and
        // the card are all lighter than the text that sits on them.
        set_theme(Theme::Light);
        for surface in [bg_app(), bg_chat(), bg_textarea(), embed_bg(), panel_bg()] {
            assert!(surface.l > 0.8, "{surface:?}");
            assert!(text().l < 0.4);
        }
        set_theme(Theme::Dark);
        for surface in [bg_app(), bg_chat(), bg_textarea(), embed_bg(), panel_bg()] {
            assert!(surface.l < 0.2, "{surface:?}");
        }
    }

    #[test]
    fn the_system_choice_follows_the_operating_system() {
        assert_eq!(Pref::System.resolve(true), Theme::Dark);
        assert_eq!(Pref::System.resolve(false), Theme::Light);
        assert_eq!(Pref::Coal.resolve(false), Theme::Coal);
        assert!(appearance_is_dark(WindowAppearance::Dark));
        assert!(appearance_is_dark(WindowAppearance::VibrantDark));
        assert!(!appearance_is_dark(WindowAppearance::Light));
        set_system_dark(false);
        apply(Pref::System);
        assert_eq!(current(), Theme::Light);
        set_system_dark(true);
        apply(Pref::System);
        assert_eq!(current(), Theme::Dark);
    }

    #[test]
    fn theme_names_round_trip_with_the_server() {
        for pref in Pref::ALL {
            assert_eq!(Pref::from_server(pref.server_name()), Some(pref));
        }
        assert_eq!(Pref::from_server("dark_legacy"), Some(Pref::Dark));
        assert_eq!(Pref::from_server(""), None);
        assert_eq!(Pref::from_server("neon"), None);
    }

    #[test]
    fn message_metrics_are_kept_in_range() {
        assert_eq!(message_font_px(), 16.);
        assert_eq!(message_line_px(), 22.);
        set_message_font_px(20.);
        assert_eq!(message_line_px(), 28.);
        set_message_font_px(f32::NAN);
        assert_eq!(message_font_px(), 16.);
        set_message_font_px(500.);
        assert_eq!(message_font_px(), 48.);
        set_message_font_px(16.);
        set_group_spacing(-3.);
        assert_eq!(group_spacing(), 0.);
        set_group_spacing(16.);
    }

    #[test]
    fn accent_override_and_reset() {
        assert_eq!(parse_hex("#ff7e00"), Some(0xff7e00));
        assert_eq!(parse_hex("ff7e00"), None);
        assert_eq!(parse_hex("#ff7e0"), None);
        set_accent(Some(0xff7e00));
        let brand_now: Rgba = brand().into();
        assert!((brand_now.r - 1.0).abs() < 0.01 && brand_now.b < 0.01);
        let link_now: Rgba = link().into();
        assert!(link_now.r > link_now.b, "the link keeps the accent's hue");
        // The link reads on the background: light on the dark themes, dark on light.
        assert!(link().l > 0.6, "light link on a dark background");
        set_theme(Theme::Light);
        assert!(link().l < 0.5, "dark link on a light background");
        set_theme(Theme::Dark);
        set_accent(None);
        let brand_default: Rgba = brand().into();
        assert!(
            brand_default.b > brand_default.r,
            "back to the default indigo"
        );
    }
}
