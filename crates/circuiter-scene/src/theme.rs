//! Colour themes shared by every renderer, so the screen, SVG export and print match.

use crate::Role;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgba(pub u8, pub u8, pub u8, pub u8);

impl Rgba {
    pub const fn hex(rgb: u32) -> Rgba {
        Rgba((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8, 255)
    }

    pub fn css(&self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub name: &'static str,
    pub dark: bool,
    pub background: Rgba,
    pub grid: Rgba,
    pub wire: Rgba,
    pub body: Rgba,
    pub pin: Rgba,
    pub pin_name: Rgba,
    pub reference: Rgba,
    pub value: Rgba,
    pub net_label: Rgba,
    pub port: Rgba,
    pub unconnected: Rgba,
    /// Selected objects.
    pub selection: Rgba,
    /// The net under inspection.
    pub highlight: Rgba,
}

impl Theme {
    /// Light theme, like a printed schematic.
    pub const PAPER: Theme = Theme {
        name: "Paper",
        dark: false,
        background: Rgba::hex(0xFBFAF6),
        grid: Rgba::hex(0xC9C5BA),
        wire: Rgba::hex(0x1F7A3A),
        body: Rgba::hex(0x8C1C13),
        pin: Rgba::hex(0x8C1C13),
        pin_name: Rgba::hex(0x3D5A80),
        reference: Rgba::hex(0x17646F),
        value: Rgba::hex(0x17646F),
        net_label: Rgba::hex(0x1A1A1A),
        port: Rgba::hex(0x6A3D9A),
        unconnected: Rgba::hex(0xE0301E),
        selection: Rgba::hex(0xE8871E),
        highlight: Rgba::hex(0x2E86DE),
    };

    /// Dark theme for long sessions at the bench.
    pub const BENCH: Theme = Theme {
        name: "Bench",
        dark: true,
        background: Rgba::hex(0x16191E),
        grid: Rgba::hex(0x4A525E),
        wire: Rgba::hex(0x5BCB7E),
        body: Rgba::hex(0xE2725B),
        pin: Rgba::hex(0xE2725B),
        pin_name: Rgba::hex(0x9FB3C8),
        reference: Rgba::hex(0x6CC4D9),
        value: Rgba::hex(0x8FB8C9),
        net_label: Rgba::hex(0xE8E6E3),
        port: Rgba::hex(0xB48EDB),
        unconnected: Rgba::hex(0xFF5A4E),
        selection: Rgba::hex(0xFFB454),
        highlight: Rgba::hex(0x4FB3FF),
    };

    pub const ALL: [&'static Theme; 2] = [&Theme::PAPER, &Theme::BENCH];

    pub fn color(&self, role: Role) -> Rgba {
        match role {
            Role::Wire | Role::Junction => self.wire,
            Role::Body => self.body,
            Role::Pin => self.pin,
            Role::PinName => self.pin_name,
            Role::Reference => self.reference,
            Role::Value => self.value,
            Role::NetLabel => self.net_label,
            Role::Port => self.port,
            Role::Unconnected => self.unconnected,
        }
    }
}
