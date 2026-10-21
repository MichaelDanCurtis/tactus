//! The device-rendered page model, used by the Kontrol S-series MK3.
//!
//! On this model the host does not compose pixels. It selects one of four
//! pages and supplies typed view-models; the keyboard's own renderer draws
//! them. See the module docs of [`crate`] for why this is a separate arm from
//! the pixel model.
//!
//! # Evidence and its limits
//!
//! Everything in [`Page`] is **recovered and verified**: the enum's four
//! variants and their discriminants come from decompiling the vendor's own
//! symbol-to-enum lookup-table builder, and the page argument is confirmed to
//! travel the wire as a single byte.
//!
//! [`PageCapability`] is weaker evidence and labelled as such. The names are
//! real observed handler symbols from the vendor's IPC mediator, but the
//! **wire** method names are not all confirmed. The transport interns method
//! names to a `uint16` index through a negotiated symbol registry, and that
//! index-to-name mapping has not been enumerated. Only `client_page_set` has
//! been observed as a literal wire string.
//!
//! Consequently this module models **what pages exist** (solid) and **what
//! capability areas exist** (solid), but does not yet claim payload schemas or
//! method indices. Those require either further static analysis of the symbol
//! registry or an observed session.

/// A page the device can display.
///
/// Discriminants are the vendor's own, recovered from its symbol-to-enum
/// lookup table. The page travels the wire as a single byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Page {
    /// Sound/preset browser.
    Browser = 0,
    /// Mixer view: track info, levels, meters.
    Mixer = 1,
    /// Plugin parameter pages.
    Parameter = 2,
    /// Smart-play view: scales, chords, key zones.
    SmartPlay = 3,
}

impl Page {
    /// Every page, in wire order.
    pub const ALL: [Page; 4] = [Page::Browser, Page::Mixer, Page::Parameter, Page::SmartPlay];

    /// The single wire byte identifying this page.
    pub const fn wire_byte(self) -> u8 {
        self as u8
    }

    /// Parse a wire byte, returning `None` for an unknown value.
    ///
    /// Total by design: a future firmware adding a fifth page must not panic
    /// a host built against this version.
    pub const fn from_wire_byte(b: u8) -> Option<Self> {
        match b {
            0 => Some(Page::Browser),
            1 => Some(Page::Mixer),
            2 => Some(Page::Parameter),
            3 => Some(Page::SmartPlay),
            _ => None,
        }
    }

    /// The vendor's symbol name for this page, as used by its enum lookup table.
    pub const fn symbol(self) -> &'static str {
        match self {
            Page::Browser => "browser",
            Page::Mixer => "mixer",
            Page::Parameter => "parameter",
            Page::SmartPlay => "smartplay",
        }
    }
}

/// The wire method that selects the active page.
///
/// The one page-related method name confirmed as a literal wire string.
pub const METHOD_PAGE_SET: &str = "client_page_set";

/// A capability area the device-rendered protocol exposes.
///
/// These correspond to observed handler symbols in the vendor's IPC mediator.
/// They describe **what content the host can supply**, grouped by the page that
/// consumes it. Payload schemas are not yet modelled — see the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PageCapability {
    /// Mixer meter levels.
    MixerMeters,
    /// Mixer track names, colours and state.
    MixerTrackInfo,
    /// Bulk mixer track data.
    MixerTrackData,
    /// The parameter page's model: names, ranges, pages.
    ParameterPageModel,
    /// A single parameter's current value.
    ParameterValue,
    /// Plugin identity and metadata for the parameter page.
    ParameterPluginData,
    /// Browser result list.
    BrowserSounds,
    /// Browser filter state.
    BrowserFilters,
    /// A single browser list row.
    BrowserSoundItem,
    /// Per-key light-guide colours.
    ///
    /// Notable: the light guide is reachable through this protocol as well as
    /// through the raw HID output report, so a host has two routes to it.
    LightGuide,
    /// Smart-play data: scales, chords, key zones.
    SmartPlayData,
    /// Plugin chain contents.
    PluginChainData,
}

impl PageCapability {
    /// The page that consumes this capability's content, where it belongs to
    /// one. [`PageCapability::LightGuide`] is page-independent.
    pub const fn page(self) -> Option<Page> {
        match self {
            Self::MixerMeters | Self::MixerTrackInfo | Self::MixerTrackData => Some(Page::Mixer),
            Self::ParameterPageModel | Self::ParameterValue | Self::ParameterPluginData => {
                Some(Page::Parameter)
            }
            Self::BrowserSounds | Self::BrowserFilters | Self::BrowserSoundItem => {
                Some(Page::Browser)
            }
            Self::SmartPlayData => Some(Page::SmartPlay),
            Self::PluginChainData => None,
            Self::LightGuide => None,
        }
    }
}

/// Deprecated alias retained for the crate's public re-export.
pub use PageCapability as PageMethod;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_discriminants_match_the_recovered_enum() {
        // Recovered from the vendor's symbol-to-enum lookup table. These four
        // values and their order are the load-bearing fact of this module.
        assert_eq!(Page::Browser.wire_byte(), 0);
        assert_eq!(Page::Mixer.wire_byte(), 1);
        assert_eq!(Page::Parameter.wire_byte(), 2);
        assert_eq!(Page::SmartPlay.wire_byte(), 3);
    }

    #[test]
    fn page_round_trips_through_its_wire_byte() {
        for p in Page::ALL {
            assert_eq!(Page::from_wire_byte(p.wire_byte()), Some(p));
        }
    }

    #[test]
    fn unknown_page_bytes_are_rejected_not_guessed() {
        // A future firmware adding a page must not panic an older host.
        for b in 4u8..=255 {
            assert_eq!(Page::from_wire_byte(b), None, "byte {b} must not map");
        }
    }

    #[test]
    fn all_is_in_wire_order_and_complete() {
        let bytes: Vec<u8> = Page::ALL.iter().map(|p| p.wire_byte()).collect();
        assert_eq!(bytes, vec![0, 1, 2, 3]);
    }

    #[test]
    fn symbols_match_the_vendor_names() {
        assert_eq!(Page::Browser.symbol(), "browser");
        assert_eq!(Page::Mixer.symbol(), "mixer");
        assert_eq!(Page::Parameter.symbol(), "parameter");
        assert_eq!(Page::SmartPlay.symbol(), "smartplay");
    }

    #[test]
    fn capabilities_map_to_the_pages_that_consume_them() {
        assert_eq!(PageCapability::MixerMeters.page(), Some(Page::Mixer));
        assert_eq!(
            PageCapability::ParameterPageModel.page(),
            Some(Page::Parameter)
        );
        assert_eq!(PageCapability::BrowserSounds.page(), Some(Page::Browser));
        assert_eq!(PageCapability::SmartPlayData.page(), Some(Page::SmartPlay));
        // The light guide is not a page's content.
        assert_eq!(PageCapability::LightGuide.page(), None);
    }

    #[test]
    fn the_page_set_method_name_is_the_confirmed_literal() {
        assert_eq!(METHOD_PAGE_SET, "client_page_set");
    }
}
