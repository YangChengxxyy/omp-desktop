//! Maps the DESIGN.md tokens onto gpui-kit's theme, following the system
//! appearance at launch.

use gpui_kit::component::theme::{Theme, ThemeMode};
use gpui_kit::*;

pub fn apply(cx: &mut App) {
	let dark = matches!(cx.window_appearance(), WindowAppearance::Dark | WindowAppearance::VibrantDark);
	Theme::change(if dark { ThemeMode::Dark } else { ThemeMode::Light }, None, cx);
	let p = if dark { &DARK } else { &LIGHT };
	Theme::update(cx, |theme| {
		theme.background = rgb(p.background).into();
		theme.foreground = rgb(p.foreground).into();
		theme.muted = rgb(p.muted).into();
		theme.muted_foreground = rgb(p.muted_foreground).into();
		theme.border = rgb(p.border).into();
		theme.accent = rgb(p.accent).into();
		theme.accent_foreground = rgb(p.accent_foreground).into();
		theme.primary = rgb(p.primary).into();
		theme.primary_foreground = rgb(0xFFFFFF).into();
		theme.danger = rgb(p.danger).into();
		theme.warning = rgb(p.warning).into();
		theme.success = rgb(p.success).into();
	});
}

/// Agent-specific tokens gpui-kit has no slot for.
#[derive(Clone, Copy)]
pub struct Palette {
	pub background:        u32,
	pub panel:             u32,
	pub elevated:          u32,
	pub code:              u32,
	pub foreground:        u32,
	pub muted:             u32,
	pub muted_foreground:  u32,
	pub border:            u32,
	pub accent:            u32,
	pub accent_foreground: u32,
	pub primary:           u32,
	pub success:           u32,
	pub warning:           u32,
	pub danger:            u32,
	pub thinking_bg:       u32,
	pub thinking_border:   u32,
}

pub const LIGHT: Palette = Palette {
	background:        0xF7F6F2,
	panel:             0xEFECE6,
	elevated:          0xFFFFFF,
	code:              0xF2F0EC,
	foreground:        0x16181D,
	muted:             0xE4E0D8,
	muted_foreground:  0x5C606B,
	border:            0xDCD8CE,
	accent:            0xE6EEFF,
	accent_foreground: 0x123BB5,
	primary:           0x1849D6,
	success:           0x137347,
	warning:           0xB25E09,
	danger:            0xC82829,
	thinking_bg:       0xF1EFEA,
	thinking_border:   0xD5D0C5,
};

pub const DARK: Palette = Palette {
	background:        0x0F1115,
	panel:             0x15181E,
	elevated:          0x1C2028,
	code:              0x0B0D10,
	foreground:        0xE6E8EE,
	muted:             0x232832,
	muted_foreground:  0x8E95A5,
	border:            0x262B36,
	accent:            0x19284D,
	accent_foreground: 0x8AB0FF,
	primary:           0x3B74FF,
	success:           0x2EA063,
	warning:           0xE5922A,
	danger:            0xF05252,
	thinking_bg:       0x14171D,
	thinking_border:   0x282E3A,
};

pub fn palette(cx: &App) -> &'static Palette {
	use gpui_kit::component::ActiveTheme as _;
	if cx.theme().is_dark() { &DARK } else { &LIGHT }
}
