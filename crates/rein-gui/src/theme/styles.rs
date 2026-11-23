// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Iced container, button, and input styles matching Claude Desktop warm dark UI

use iced::border::Radius;
use iced::widget::{button, container, text_input};
use iced::{Background, Border, Color, Theme};

use super::palette::{
    ACCENT_CORAL, BG_CANVAS, BG_CODE, BG_ELEVATED, BG_INSPECTOR, BG_SIDEBAR, BG_SURFACE,
    BORDER_SOFT, BORDER_SUBTLE, STATUS_RED, TEXT_MUTED, TEXT_PRIMARY,
};

/// Main canvas container style.
#[must_use]
pub fn canvas_container(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BG_CANVAS)),
        text_color: Some(TEXT_PRIMARY),
        ..Default::default()
    }
}

/// Collapsible left sidebar container style.
#[must_use]
pub fn sidebar_container(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BG_SIDEBAR)),
        border: Border {
            color: BORDER_SOFT,
            width: 1.0,
            radius: Radius::default(),
        },
        text_color: Some(TEXT_PRIMARY),
        ..Default::default()
    }
}

/// Right-hand split inspector drawer style (Pasted content / Code / Tool output).
#[must_use]
pub fn inspector_container(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BG_INSPECTOR)),
        border: Border {
            color: BORDER_SOFT,
            width: 1.0,
            radius: Radius::default(),
        },
        text_color: Some(TEXT_PRIMARY),
        ..Default::default()
    }
}

/// Sleek rounded composer card style (`16px` radius, soft 1px border).
#[must_use]
pub fn composer_card(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BG_SURFACE)),
        border: Border {
            color: BORDER_SOFT,
            width: 1.0,
            radius: 16.0.into(),
        },
        text_color: Some(TEXT_PRIMARY),
        ..Default::default()
    }
}

/// Rounded user message bubble style (`16px` radius, warm surface, borderless).
#[must_use]
pub fn user_bubble(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BG_SURFACE)),
        border: Border {
            color: BORDER_SOFT,
            width: 0.5,
            radius: 16.0.into(),
        },
        text_color: Some(TEXT_PRIMARY),
        ..Default::default()
    }
}

/// Rounded card container style.
#[must_use]
pub fn surface_card(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BG_SURFACE)),
        border: Border {
            color: BORDER_SUBTLE,
            width: 1.0,
            radius: 14.0.into(),
        },
        text_color: Some(TEXT_PRIMARY),
        ..Default::default()
    }
}

/// Compact `PASTED` attachment badge container style.
#[must_use]
pub fn pasted_badge_card(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BG_SURFACE)),
        border: Border {
            color: BORDER_SUBTLE,
            width: 1.0,
            radius: 12.0.into(),
        },
        text_color: Some(TEXT_PRIMARY),
        ..Default::default()
    }
}

/// Elevated popover card (`+` menu, model picker, approval modal).
#[must_use]
pub fn elevated_card(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BG_ELEVATED)),
        border: Border {
            color: BORDER_SUBTLE,
            width: 1.0,
            radius: 12.0.into(),
        },
        text_color: Some(TEXT_PRIMARY),
        ..Default::default()
    }
}

/// Muted tool / thinking output card.
#[must_use]
pub fn disclosure_card(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BG_CODE)),
        border: Border {
            color: BORDER_SOFT,
            width: 1.0,
            radius: 10.0.into(),
        },
        text_color: Some(TEXT_MUTED),
        ..Default::default()
    }
}

/// Circular coral user avatar badge in the sidebar footer.
#[must_use]
pub fn avatar_badge(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(ACCENT_CORAL)),
        border: Border {
            radius: 999.0.into(),
            ..Default::default()
        },
        text_color: Some(Color::WHITE),
        ..Default::default()
    }
}

/// Subtle ghost button (sidebar items, topbar icons, disclosure headers).
#[must_use]
pub fn ghost_button(_theme: &Theme, status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered | button::Status::Pressed => Some(Background::Color(BG_ELEVATED)),
        _ => None,
    };
    button::Style {
        background: bg,
        text_color: TEXT_PRIMARY,
        border: Border {
            radius: 8.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Active sidebar session row button.
#[must_use]
pub fn active_sidebar_button(_theme: &Theme, status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered | button::Status::Pressed => BG_ELEVATED,
        _ => BG_SURFACE,
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: TEXT_PRIMARY,
        border: Border {
            radius: 8.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Pill button (mode badge, quick-action chip, model picker item).
#[must_use]
pub fn pill_button(_theme: &Theme, status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered | button::Status::Pressed => BG_ELEVATED,
        _ => BG_SURFACE,
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: TEXT_PRIMARY,
        border: Border {
            color: BORDER_SOFT,
            width: 1.0,
            radius: 10.0.into(),
        },
        ..Default::default()
    }
}

/// Primary amber-gold action button (Send, Approve).
#[must_use]
pub fn primary_button(_theme: &Theme, status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered => Color::from_rgb(0.945, 0.725, 0.310),
        button::Status::Disabled => BORDER_SUBTLE,
        _ => ACCENT_CORAL,
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: Color::from_rgb(0.067, 0.075, 0.090),
        border: Border {
            radius: 10.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Danger button (Stop, Deny).
#[must_use]
pub fn danger_button(_theme: &Theme, status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered => Color::from_rgb(0.95, 0.48, 0.48),
        _ => STATUS_RED,
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: Color::WHITE,
        border: Border {
            radius: 10.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Borderless composer text input style.
#[must_use]
pub fn composer_input(_theme: &Theme, _status: text_input::Status) -> text_input::Style {
    text_input::Style {
        background: Background::Color(Color::TRANSPARENT),
        border: Border::default(),
        icon: TEXT_MUTED,
        placeholder: TEXT_MUTED,
        value: TEXT_PRIMARY,
        selection: Color::from_rgba(0.898, 0.663, 0.235, 0.30),
    }
}
