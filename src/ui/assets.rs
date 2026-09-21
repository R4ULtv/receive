//! Icons available to the interface.
//!
//! `gpui_kit::assets::Assets` embeds the component icon set. Receive needs a
//! handful of mail-specific icons from the wider Lucide catalog and its own
//! wordmark glyph, so it composes both with the component set instead of
//! embedding all 1,830 bundled SVGs.

use gpui_kit::{AssetSource, Result, SharedString};
use std::borrow::Cow;

gpui_kit::assets::icon_assets!(
    MailIcons,
    [
        Archive,
        ArchiveRestore,
        AtSign,
        Clock,
        KeyRound,
        Mail,
        MailOpen,
        Paperclip,
        Reply,
        Send,
        ShieldCheck,
        SquarePen,
        X,
    ]
);

/// The mail glyph from `assets/receive.svg`, drawn wherever the app names itself.
pub const MARK: &str = "icons/receive-mark.svg";
const MARK_SVG: &[u8] = include_bytes!("../../assets/icons/receive.svg");

/// The component icons, the extra mail icons, and the wordmark glyph.
#[derive(Clone, Copy, Debug, Default)]
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path == MARK {
            return Ok(Some(Cow::Borrowed(MARK_SVG)));
        }
        if let Some(bytes) = MailIcons.load(path)? {
            return Ok(Some(bytes));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(MailIcons.list(path)?);
        if MARK.starts_with(path) {
            paths.push(MARK.into());
        }
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::assets::IconName;

    /// Every icon the interface draws. An icon outside the embedded set renders
    /// as empty space instead of failing, which is easy to miss by eye, so the
    /// whole list is resolved here.
    const DRAWN: &[IconName] = &[
        IconName::Archive,
        IconName::ArchiveRestore,
        IconName::AtSign,
        IconName::CircleCheck,
        IconName::Clock,
        IconName::ExternalLink,
        IconName::Eye,
        IconName::Globe,
        IconName::HardDrive,
        IconName::Inbox,
        IconName::KeyRound,
        IconName::Mail,
        IconName::MailOpen,
        IconName::Paperclip,
        IconName::Reply,
        IconName::RotateCw,
        IconName::Search,
        IconName::Send,
        IconName::Settings,
        IconName::ShieldCheck,
        IconName::SquarePen,
        IconName::TriangleAlert,
        IconName::X,
    ];

    #[test]
    fn every_icon_the_interface_draws_is_embedded() {
        for icon in DRAWN {
            let path = icon.path();
            assert!(
                Assets.load(&path).unwrap().is_some(),
                "{path} is not embedded: add it to the icon_assets! list"
            );
        }
        assert!(Assets.load(MARK).unwrap().is_some(), "{MARK} is missing");
    }
}
