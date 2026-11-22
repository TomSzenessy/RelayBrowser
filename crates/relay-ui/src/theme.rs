//! Colour tokens from docs/design.md; one place so light and dark stay in step.

use gpui::{rgb, rgba, Hsla, WindowAppearance};

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub background: Hsla,
    pub surface: Hsla,
    pub page: Hsla,
    pub border: Hsla,
    pub text: Hsla,
    pub muted: Hsla,
    pub code: Hsla,
    pub accent: Hsla,
    pub active: Hsla,
    pub hover: Hsla,
    pub agent: Hsla,
    pub agent_bg: Hsla,
    pub error: Hsla,
    pub warn: Hsla,
    pub scrim: Hsla,
}

impl Theme {
    pub fn for_appearance(a: WindowAppearance) -> Self {
        match a {
            WindowAppearance::Light | WindowAppearance::VibrantLight => Self::light(),
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::dark(),
        }
    }

    pub fn dark() -> Self {
        Self {
            background: rgb(0x1b1c1f).into(),
            surface: rgb(0x232428).into(),
            page: rgb(0x1f2023).into(),
            border: rgb(0x34363b).into(),
            text: rgb(0xe6e6e9).into(),
            muted: rgb(0x9a9ca3).into(),
            code: rgb(0xc9d1ff).into(),
            accent: rgb(0x5b8cff).into(),
            active: rgba(0x5b8cff33).into(),
            hover: rgba(0xffffff12).into(),
            agent: rgb(0xb07cff).into(),
            agent_bg: rgba(0xb07cff2e).into(),
            error: rgb(0xff6b6b).into(),
            warn: rgb(0xffb84d).into(),
            scrim: rgba(0x0000004d).into(),
        }
    }

    pub fn light() -> Self {
        Self {
            background: rgb(0xf6f6f7).into(),
            surface: rgb(0xffffff).into(),
            page: rgb(0xfbfbfc).into(),
            border: rgb(0xe2e2e6).into(),
            text: rgb(0x1c1c1f).into(),
            muted: rgb(0x6b6d75).into(),
            code: rgb(0x2a3a8f).into(),
            accent: rgb(0x2f6bff).into(),
            active: rgba(0x2f6bff24).into(),
            hover: rgba(0x0000000d).into(),
            agent: rgb(0x7a3cff).into(),
            agent_bg: rgba(0x7a3cff1f).into(),
            error: rgb(0xd93636).into(),
            warn: rgb(0xb26b00).into(),
            scrim: rgba(0x00000026).into(),
        }
    }
}
