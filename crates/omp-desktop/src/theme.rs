//! Maps the DESIGN.md tokens onto gpui-kit's theme. The user picks light,
//! dark, or "follow the system"; the last one tracks appearance changes
//! live (see `Workspace`). The choice is saved across launches.

use std::path::PathBuf;

use gpui_kit::component::theme::{Theme, ThemeMode};
use gpui_kit::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemeChoice {
	#[default]
	System,
	Light,
	Dark,
}

impl ThemeChoice {
	pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];

	pub fn label(self) -> &'static str {
		match self {
			Self::System => "跟随系统",
			Self::Light => "浅色",
			Self::Dark => "深色",
		}
	}

	fn key(self) -> &'static str {
		match self {
			Self::System => "system",
			Self::Light => "light",
			Self::Dark => "dark",
		}
	}

	fn parse(text: &str) -> Option<Self> {
		Self::ALL.into_iter().find(|c| c.key() == text.trim())
	}

	fn is_dark(self, appearance: WindowAppearance) -> bool {
		match self {
			Self::System => matches!(appearance, WindowAppearance::Dark | WindowAppearance::VibrantDark),
			Self::Light => false,
			Self::Dark => true,
		}
	}
}

struct Choice(ThemeChoice);

impl Global for Choice {}

pub fn choice(cx: &App) -> ThemeChoice {
	cx.try_global::<Choice>().map(|c| c.0).unwrap_or_default()
}

/// Startup: the saved choice against the system appearance.
pub fn apply(cx: &mut App) {
	let choice = if cfg!(test) { ThemeChoice::System } else { load().unwrap_or_default() };
	cx.set_global(Choice(choice));
	let appearance = cx.window_appearance();
	paint(choice.is_dark(appearance), cx);
}

/// The window appearance changed; matters only when following the system.
pub fn appearance_changed(window: &mut Window, cx: &mut App) {
	if choice(cx) == ThemeChoice::System {
		paint(ThemeChoice::System.is_dark(window.appearance()), cx);
		window.refresh();
	}
}

/// A pick from the theme menu: applies and saves it.
pub fn set_choice(choice: ThemeChoice, window: &mut Window, cx: &mut App) {
	cx.set_global(Choice(choice));
	paint(choice.is_dark(window.appearance()), cx);
	window.refresh();
	if !cfg!(test) {
		save(choice);
	}
}

fn settings_file() -> Option<PathBuf> {
	let home = std::env::var_os("HOME")?;
	Some(PathBuf::from(home).join("Library/Application Support/omp-desktop/theme"))
}

fn load() -> Option<ThemeChoice> {
	ThemeChoice::parse(&std::fs::read_to_string(settings_file()?).ok()?)
}

fn save(choice: ThemeChoice) {
	let Some(file) = settings_file() else {
		return;
	};
	if let Some(dir) = file.parent() {
		_ = std::fs::create_dir_all(dir);
	}
	if let Err(error) = std::fs::write(&file, choice.key()) {
		eprintln!("omp-desktop: saving the theme to {}: {error}", file.display());
	}
}

fn paint(dark: bool, cx: &mut App) {
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

#[cfg(test)]
mod tests {
	use super::{ThemeChoice, WindowAppearance};

	#[test]
	fn choice_round_trips_and_resolves() {
		for choice in ThemeChoice::ALL {
			assert_eq!(ThemeChoice::parse(choice.key()), Some(choice));
		}
		assert_eq!(ThemeChoice::parse("dark\n"), Some(ThemeChoice::Dark));
		assert_eq!(ThemeChoice::parse("blue"), None);
		assert!(ThemeChoice::System.is_dark(WindowAppearance::VibrantDark));
		assert!(!ThemeChoice::System.is_dark(WindowAppearance::Light));
		assert!(ThemeChoice::Dark.is_dark(WindowAppearance::Light));
		assert!(!ThemeChoice::Light.is_dark(WindowAppearance::Dark));
	}
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
