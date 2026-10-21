//! The logo: the mark (a P of test bars with a cursor, "B+C 2" on the design
//! canvas) and the lowercase wordmark. Animations come from pedagog.css
//! (`mk-anim` builds the bars in, `mk-loop` loops them while loading,
//! `wm-shuffle` deals the letters through seven faces), each with a
//! reduced-motion fallback there.

use leptos::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    Still,
    /// Builds in once (first load, the top bar).
    BuildIn,
    /// Loops while something loads.
    Loading,
}

/// The mark, `size` px square, in the text colour with accent bars.
#[component]
pub fn Mark(
    #[prop(default = 26)] size: u32,
    #[prop(default = Motion::Still)] motion: Motion,
) -> impl IntoView {
    let class = match motion {
        Motion::Still => "",
        Motion::BuildIn => "mk-anim",
        Motion::Loading => "mk-loop",
    };
    view! {
        <svg class=class viewBox="0 0 64 64" width=size height=size aria-hidden="true" style="flex: none; display: block">
            <rect class="stem" x="11" y="10" width="10" height="44" rx="3.5" style="fill: currentColor"></rect>
            <rect class="bar b1" x="25" y="10" width="28" height="10" rx="3.5" style="fill: var(--accent-text)"></rect>
            <rect class="bar b2" x="25" y="24" width="28" height="10" rx="3.5" style="fill: var(--accent-text)"></rect>
            <rect class="bar cursor" x="25" y="38" width="9" height="14" rx="2" style="fill: currentColor"></rect>
        </svg>
    }
}

/// The letters' shuffle orders (three, so neighbours never match) and delays.
pub fn letters() -> Vec<(char, usize, String)> {
    "pedagog"
        .chars()
        .enumerate()
        .map(|(i, c)| (c, i % 3, format!("{:.2}s", 0.25 + i as f64 * 0.09)))
        .collect()
}

/// "pedagog" in Plex Sans Bold, in em: the letters' advances less the tracking.
const WORD_EM: f64 = 3.84;

/// The lockup's final width at `size`: the mark, the gap, the word.
pub fn lockup_width(size: u32) -> f64 {
    let size = f64::from(size);
    size + 12.0 + size * 0.74 * WORD_EM
}

/// The mark and the word, as the design lays them out: the word's size is
/// 0.74 of the mark's, its top on the mark's top. The lockup is as wide as
/// its settled word, so while the letters shuffle the mark stays put and
/// only the word's right edge moves.
#[component]
pub fn Lockup(
    #[prop(default = 40)] size: u32,
    #[prop(default = false)] shuffle: bool,
) -> impl IntoView {
    let font = format!(
        "font-size: {:.1}px; line-height: 1; margin-top: {:.1}px; font-weight: 700; letter-spacing: -0.025em; white-space: nowrap",
        f64::from(size) * 0.74,
        f64::from(size) * 0.0375
    );
    let word = if shuffle {
        view! {
            <span class="wm wm-shuffle" aria-label="pedagog" style=font>
                {letters()
                    .into_iter()
                    .map(|(c, v, delay)| view! {
                        <span class="slot">
                            <span class=format!("ch v{v}") aria-hidden="true" style=format!("animation-delay: {delay}")>{c.to_string()}</span>
                        </span>
                    })
                    .collect_view()}
            </span>
        }
        .into_any()
    } else {
        view! { <span style=font>"pedagog"</span> }.into_any()
    };
    view! {
        <div style=format!("display: flex; align-items: flex-start; gap: 12px; width: {:.1}px; color: var(--text)", lockup_width(size))>
            <Mark size=size motion=if shuffle { Motion::BuildIn } else { Motion::Still }/>
            {word}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_take_turns_and_stagger() {
        let l = letters();
        assert_eq!(l.iter().map(|(c, _, _)| c).collect::<String>(), "pedagog");
        assert_eq!(
            l.iter().map(|(_, v, _)| *v).collect::<Vec<_>>(),
            [0, 1, 2, 0, 1, 2, 0]
        );
        assert_eq!(l[0].2, "0.25s");
        assert_eq!(l[6].2, "0.79s");
    }

    #[test]
    fn the_lockup_is_as_wide_as_the_design_draws_it() {
        assert_eq!(lockup_width(40).round(), 166.0);
    }
}

#[cfg(all(test, target_arch = "wasm32"))]
mod browser {
    use wasm_bindgen_test::*;

    use super::*;
    use crate::testing::{all, mount};

    wasm_bindgen_test_configure!(run_in_browser);

    #[wasm_bindgen_test]
    fn the_mark_is_four_bars_and_animates_as_asked() {
        let still = mount(|| view! { <Mark size=26/> });
        assert_eq!(all(&still, "rect").len(), 4);
        assert_eq!(
            all(&still, "svg")[0]
                .get_attribute("class")
                .unwrap_or_default(),
            ""
        );
        let building = mount(|| view! { <Mark motion=Motion::BuildIn/> });
        assert_eq!(all(&building, "svg.mk-anim").len(), 1);
        let loading = mount(|| view! { <Mark motion=Motion::Loading/> });
        assert_eq!(all(&loading, "svg.mk-loop").len(), 1);
        assert_eq!(
            all(&loading, "svg")[0]
                .get_attribute("aria-hidden")
                .as_deref(),
            Some("true")
        );
    }

    #[wasm_bindgen_test]
    fn the_shuffle_deals_seven_letters_in_three_orders() {
        let lockup = mount(|| view! { <Lockup size=40 shuffle=true/> });
        let letters = all(&lockup, ".wm-shuffle .ch");
        assert_eq!(letters.len(), 7);
        assert_eq!(all(&lockup, ".ch.v2").len(), 2);
        assert_eq!(
            all(&lockup, ".wm-shuffle")[0]
                .get_attribute("aria-label")
                .as_deref(),
            Some("pedagog")
        );
    }
}
