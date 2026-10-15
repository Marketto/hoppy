//! The panel icon: a StatusNotifierItem (KDE/freedesktop spec, `ksni`), shown by KDE,
//! Ubuntu's AppIndicator GNOME extension, Cinnamon, XFCE (with the plugin)… The menu is
//! persistent (the host draws it): `gui.rs` keeps it current with `Tray::set_menu`, and a
//! pick arrives as `TrayEvent::Menu(id)`. When the host appears later (GNOME loads its
//! extension after autostart apps) ksni registers then; without a host the icon simply
//! does not show and Hoppy keeps receiving.

use std::sync::mpsc::Sender;

use anyhow::{Result, anyhow};
use ksni::blocking::{Handle, TrayMethods};
use ksni::menu::{CheckmarkItem, StandardItem};

use crate::diag;
use crate::platform::{MenuItem, TrayEvent};

static ICON_48: &[u8] = include_bytes!("../../../../assets/brand/linux/hoppy-48.png");
static ICON_128: &[u8] = include_bytes!("../../../../assets/brand/linux/hoppy-128.png");

/// Owned copy of a `MenuItem` (the host may ask for the menu at any time).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Action(u32, String),
    Check(u32, String, bool),
    /// Highlighted action (the update entry): shown with an icon.
    Bold(u32, String),
    Label(String),
    Separator,
}

impl Item {
    pub fn from_menu(item: &MenuItem<'_>) -> Item {
        match *item {
            MenuItem::Action(id, t) => Item::Action(id, mnemonic_safe(t)),
            MenuItem::Check(id, t, on) => Item::Check(id, mnemonic_safe(t), on),
            MenuItem::Bold(id, t) => Item::Bold(id, mnemonic_safe(t)),
            MenuItem::Label(t) => Item::Label(mnemonic_safe(t)),
            MenuItem::Separator => Item::Separator,
        }
    }
}

/// dbusmenu labels use `_` for keyboard mnemonics: show names like "my_pc" literally.
pub fn mnemonic_safe(s: &str) -> String {
    s.replace('_', "__")
}

struct PanelItem {
    tip: String,
    items: Vec<Item>,
    icons: Vec<ksni::Icon>,
    tx: Sender<TrayEvent>,
}

impl ksni::Tray for PanelItem {
    // Left click opens the menu too (KDE would otherwise call `activate`), as on Windows.
    const MENU_ON_ACTIVATE: bool = true;

    fn id(&self) -> String {
        "hoppy".into()
    }

    fn title(&self) -> String {
        "Hoppy".into()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.icons.clone()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: self.tip.clone(),
            ..Default::default()
        }
    }

    fn menu_about_to_show(&mut self) {
        let _ = self.tx.send(TrayEvent::MenuOpening);
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        self.items
            .iter()
            .map(|item| match item {
                Item::Action(id, label) | Item::Bold(id, label) => {
                    let id = *id;
                    StandardItem {
                        label: label.clone(),
                        icon_name: if matches!(item, Item::Bold(..)) {
                            "software-update-available".into()
                        } else {
                            String::new()
                        },
                        activate: Box::new(move |t: &mut Self| {
                            let _ = t.tx.send(TrayEvent::Menu(id));
                        }),
                        ..Default::default()
                    }
                    .into()
                }
                Item::Check(id, label, on) => {
                    let id = *id;
                    CheckmarkItem {
                        label: label.clone(),
                        checked: *on,
                        activate: Box::new(move |t: &mut Self| {
                            let _ = t.tx.send(TrayEvent::Menu(id));
                        }),
                        ..Default::default()
                    }
                    .into()
                }
                Item::Label(label) => StandardItem {
                    label: label.clone(),
                    enabled: false,
                    ..Default::default()
                }
                .into(),
                Item::Separator => ksni::MenuItem::Separator,
            })
            .collect()
    }

    fn watcher_online(&self) {
        diag::log("tray: panel icon host online");
    }

    fn watcher_offline(&self, reason: ksni::OfflineReason) -> bool {
        diag::log(format!(
            "tray: no panel icon host ({reason:?}); notifications only"
        ));
        true // keep the service: the host may come (back) later
    }
}

pub struct Sni {
    handle: Handle<PanelItem>,
}

impl Sni {
    pub fn spawn(tip: &str, tx: Sender<TrayEvent>) -> Result<Sni> {
        let item = PanelItem {
            tip: tip.to_owned(),
            items: Vec::new(),
            icons: [ICON_48, ICON_128]
                .iter()
                .filter_map(|png| decode_icon(png))
                .collect(),
            tx,
        };
        let handle = item
            .assume_sni_available(true)
            .spawn()
            .map_err(|e| anyhow!("cannot create the panel icon: {e}"))?;
        Ok(Sni { handle })
    }

    pub fn set_tip(&self, tip: &str) {
        let tip = tip.to_owned();
        self.handle.update(move |t| t.tip = tip);
    }

    pub fn set_menu(&self, items: Vec<Item>) {
        self.handle.update(move |t| {
            if t.items != items {
                t.items = items;
            }
        });
    }
}

impl Drop for Sni {
    fn drop(&mut self) {
        self.handle.shutdown().wait();
    }
}

/// PNG → SNI pixmap (ARGB32, network byte order).
pub fn decode_icon(png_bytes: &[u8]) -> Option<ksni::Icon> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(png_bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => buf[..info.buffer_size()].to_vec(),
        png::ColorType::Rgb => buf[..info.buffer_size()]
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|&[r, g, b]| [r, g, b, 255])
            .collect(),
        _ => return None,
    };
    let data = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|&[r, g, b, a]| [a, r, g, b])
        .collect();
    Some(ksni::Icon {
        width: i32::try_from(info.width).ok()?,
        height: i32::try_from(info.height).ok()?,
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_icons_decode_to_argb() {
        let icon = decode_icon(ICON_48).expect("48 px icon");
        assert_eq!((icon.width, icon.height), (48, 48));
        assert_eq!(icon.data.len(), 48 * 48 * 4);
        // Rounded corner: fully transparent; centre: opaque.
        assert_eq!(icon.data[0], 0);
        let centre = (24 * 48 + 24) * 4;
        assert_eq!(icon.data[centre], 255);
        assert_eq!(decode_icon(ICON_128).map(|i| i.width), Some(128));
        assert!(decode_icon(b"not a png").is_none());
    }

    #[test]
    fn menu_text_keeps_underscores() {
        assert_eq!(mnemonic_safe("Hoppy · my_pc"), "Hoppy · my__pc");
        assert_eq!(
            Item::from_menu(&MenuItem::Check(4, "♪ a_b", true)),
            Item::Check(4, "♪ a__b".into(), true)
        );
    }
}
