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
    pub bus: Rgba,
    pub note: Rgba,
    /// The sheet's border and title block.
    pub frame: Rgba,
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
        background: Rgba::hex(0xF8FAFC),
        grid: Rgba::hex(0xB9C8D6),
        wire: Rgba::hex(0x187D72),
        body: Rgba::hex(0x33465C),
        pin: Rgba::hex(0x33465C),
        pin_name: Rgba::hex(0x3D5A80),
        reference: Rgba::hex(0x2463A0),
        value: Rgba::hex(0x60748A),
        net_label: Rgba::hex(0x1A1A1A),
        port: Rgba::hex(0x6A3D9A),
        unconnected: Rgba::hex(0xE0301E),
        bus: Rgba::hex(0x1F4E9A),
        note: Rgba::hex(0x4A4A4A),
        frame: Rgba::hex(0x74869A),
        selection: Rgba::hex(0xD48116),
        highlight: Rgba::hex(0x2E86DE),
    };

    /// Dark theme for long sessions at the bench: light parts and green wires on deep
    /// navy.
    pub const BENCH: Theme = Theme {
        name: "Bench",
        dark: true,
        background: Rgba::hex(0x0F1722),
        grid: Rgba::hex(0x34465C),
        wire: Rgba::hex(0x63D9AB),
        body: Rgba::hex(0xD3DBE6),
        pin: Rgba::hex(0xAAB6C6),
        pin_name: Rgba::hex(0x8DA0B6),
        reference: Rgba::hex(0xE4EAF2),
        value: Rgba::hex(0xA9BACD),
        net_label: Rgba::hex(0x5BE38F),
        port: Rgba::hex(0xA98BFA),
        unconnected: Rgba::hex(0xF87171),
        bus: Rgba::hex(0x60A5FA),
        note: Rgba::hex(0xB4C0CF),
        frame: Rgba::hex(0x6B7E98),
        selection: Rgba::hex(0xFBBF24),
        highlight: Rgba::hex(0x3B8BF6),
    };

    pub const ALL: [&'static Theme; 2] = [&Theme::PAPER, &Theme::BENCH];

    pub fn color(&self, role: Role) -> Rgba {
        match role {
            Role::Wire | Role::Junction => self.wire,
            Role::Body => self.body,
            Role::Pin => self.pin,
            Role::PinName => self.pin_name,
            Role::PinNumber => self.pin,
            Role::Reference => self.reference,
            Role::Value => self.value,
            Role::NetLabel => self.net_label,
            Role::Port => self.port,
            Role::Unconnected => self.unconnected,
            Role::Guide => self.grid,
            Role::Bus => self.bus,
            Role::Note => self.note,
            Role::Frame => self.frame,
        }
    }
}
