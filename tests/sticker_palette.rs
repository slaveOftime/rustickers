//! Smoke tests for the UI-owned sticker palette module.
//!
//! The model layer (`model::sticker::StickerColor::bg` / `swatch`) is
//! expected to delegate to this module, so changes to the surface
//! colours flow through this single boundary. These tests verify that
//! each variant resolves to a non-default, mutually-distinct colour over
//! the closed [`StickerColor::ALL`] set, and that the model delegation
//! agrees with the module's own palette.
//!
//! UI integration tests that mount real windows are planned under the
//! `#[gpui_kit::test]` framework once `gpui-kit`'s `test-support` feature
//! resolves against the platform's transitive dependency set. See
//! `docs/plans/gpui-kit-upgrade-goal.md`, milestone 6.

#[cfg(feature = "ui")]
use rustickers::model::sticker::StickerColor;

#[cfg(feature = "ui")]
mod ui_tests {
    use super::*;

    fn is_distinct(a: rustickers::native::theme::Rgba, b: rustickers::native::theme::Rgba) -> bool {
        (a.r - b.r).abs() > 1e-3
            || (a.g - b.g).abs() > 1e-3
            || (a.b - b.b).abs() > 1e-3
            || (a.a - b.a).abs() > 1e-3
    }

    fn is_equal(a: rustickers::native::theme::Rgba, b: rustickers::native::theme::Rgba) -> bool {
        (a.r - b.r).abs() < 1e-6
            && (a.g - b.g).abs() < 1e-6
            && (a.b - b.b).abs() < 1e-6
            && (a.a - b.a).abs() < 1e-6
    }

    #[test]
    fn every_sticker_color_resolves_to_a_well_formed_palette_entry() {
        for &color in StickerColor::ALL.iter() {
            let bg = color.bg();
            let swatch = color.swatch();
            assert!(
                bg.r < 0.3 && bg.g < 0.3 && bg.b < 0.3,
                "background too bright for {color:?}: r={} g={} b={}",
                bg.r,
                bg.g,
                bg.b
            );
            assert!(
                swatch.r > 0.4 || swatch.g > 0.4 || swatch.b > 0.4,
                "swatch too dark for {color:?}: r={} g={} b={}",
                swatch.r,
                swatch.g,
                swatch.b
            );
        }
    }

    #[test]
    fn every_sticker_color_has_a_distinct_background_and_swatch() {
        let bgs: Vec<_> = StickerColor::ALL.iter().map(|c| c.bg()).collect();
        let swatches: Vec<_> = StickerColor::ALL.iter().map(|c| c.swatch()).collect();

        for (i, a) in bgs.iter().enumerate() {
            for (j, b) in bgs.iter().enumerate() {
                if i == j {
                    continue;
                }
                assert!(
                    is_distinct(*a, *b),
                    "background {i:?} and {j:?} collide: {a:?} vs {b:?}"
                );
            }
        }
        for (i, a) in swatches.iter().enumerate() {
            for (j, b) in swatches.iter().enumerate() {
                if i == j {
                    continue;
                }
                assert!(
                    is_distinct(*a, *b),
                    "swatch {i:?} and {j:?} collide: {a:?} vs {b:?}"
                );
            }
        }
    }

    #[test]
    fn model_layer_agrees_with_the_ui_owned_palette() {
        for &color in StickerColor::ALL.iter() {
            let model_bg = color.bg();
            let theme_bg = rustickers::native::theme::sticker_background(color);
            assert!(
                is_equal(model_bg, theme_bg),
                "model::StickerColor::bg drifted from the UI palette for {color:?}: model={model_bg:?} theme={theme_bg:?}"
            );

            let model_swatch = color.swatch();
            let theme_swatch = rustickers::native::theme::sticker_swatch(color);
            assert!(
                is_equal(model_swatch, theme_swatch),
                "model::StickerColor::swatch drifted from the UI palette for {color:?}: model={model_swatch:?} theme={theme_swatch:?}"
            );
        }
    }
}
