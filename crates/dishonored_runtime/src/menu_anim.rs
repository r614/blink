//! The menus' ActionScript motion: `gfx.motion.Tween` (`clip.tweenTo(seconds, props, ease)`),
//! the `mx.transitions.easing` curves, `setInterval` delays and `gotoAndPlay` timelines, as a
//! small clock-driven model the screen builders read.
//!
//! Decoded from the movies' class sprites (`UI_MainMenu.MainMenu` 249 `gfx.motion.Tween`, 259
//! `Strong`, 260 `Back`, 268 `None`; the same classes are compiled into every menu movie):
//! - `tweenTo(d, props, ease)` records the clip's current value of each named property and
//!   interpolates `start + ease(t) * (target - start)` with `t = elapsed / d` in real time
//!   (`getTimer()`), then jumps to the target. A clip has one tween slot: a new `tweenTo`
//!   replaces the running one (properties only the old one named stay where they were); a
//!   plain assignment during a tween is overwritten by the tween on its next frame.
//!   `tweenEnd(false)` freezes the tween.
//! - `Strong` is quintic: `easeIn = t^5`, `easeOut = 1 - (1-t)^5`, `easeInOut` piecewise.
//! - `Back` overshoots with `s = 1.70158` (`easeInOut` uses `s * 1.525`).
//! - `setInterval(obj, "Fn", ms)` runs `Fn` after `ms` (every class clears it in `Fn`).
//!
//! Screens queue what the scripts do (property sets, tweens, timeline jumps) at the times the
//! scripts' intervals run them, and read the clip values back when they build.

use crate::menu_art::Props;
use std::collections::HashMap;

/// `mx.transitions.easing.*` (and `gfx.motion.Tween.linearEase` when none is given).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ease {
    None,
    StrongIn,
    StrongOut,
    StrongInOut,
    BackIn,
    BackOut,
    BackInOut,
}

impl Ease {
    /// The curve at `t` in 0..1 (`ease(t, 0, 1, 1)`).
    pub fn at(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        const S: f32 = 1.70158;
        match self {
            Ease::None => t,
            Ease::StrongIn => t.powi(5),
            Ease::StrongOut => (t - 1.0).powi(5) + 1.0,
            Ease::StrongInOut => {
                let u = t * 2.0;
                if u < 1.0 { 0.5 * u.powi(5) } else { 0.5 * ((u - 2.0).powi(5) + 2.0) }
            }
            Ease::BackIn => t * t * ((S + 1.0) * t - S),
            Ease::BackOut => {
                let u = t - 1.0;
                u * u * ((S + 1.0) * u + S) + 1.0
            }
            Ease::BackInOut => {
                let s = S * 1.525;
                let u = t * 2.0;
                if u < 1.0 {
                    0.5 * (u * u * ((s + 1.0) * u - s))
                } else {
                    let v = u - 2.0;
                    0.5 * (v * v * ((s + 1.0) * v + s) + 2.0)
                }
            }
        }
    }
}

/// Number of tweenable numeric properties: x, y, xscale, yscale, rotation, alpha.
const N: usize = 6;

fn props_get(p: &Props, i: usize) -> f32 {
    match i {
        0 => p.x,
        1 => p.y,
        2 => p.xscale,
        3 => p.yscale,
        4 => p.rotation,
        _ => p.alpha,
    }
}

fn props_set(p: &mut Props, i: usize, v: f32) {
    match i {
        0 => p.x = v,
        1 => p.y = v,
        2 => p.xscale = v,
        3 => p.yscale = v,
        4 => p.rotation = v,
        _ => p.alpha = v,
    }
}

/// A partial property set: the `props` object handed to `tweenTo` or assigned by script.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct P {
    /// x, y, xscale, yscale, rotation, alpha.
    pub v: [Option<f32>; N],
    pub visible: Option<bool>,
}

impl P {
    pub fn new() -> Self {
        Self::default()
    }
    /// Every property of `p` (a saved `props` object such as `_start_props`).
    pub fn all(p: Props) -> Self {
        let mut r = Self::default();
        for i in 0..N {
            r.v[i] = Some(props_get(&p, i));
        }
        r
    }
    pub fn x(mut self, v: f32) -> Self {
        self.v[0] = Some(v);
        self
    }
    pub fn y(mut self, v: f32) -> Self {
        self.v[1] = Some(v);
        self
    }
    pub fn xy(self, x: f32, y: f32) -> Self {
        self.x(x).y(y)
    }
    /// `_xscale` and `_yscale` together.
    pub fn scale(mut self, v: f32) -> Self {
        self.v[2] = Some(v);
        self.v[3] = Some(v);
        self
    }
    pub fn rot(mut self, v: f32) -> Self {
        self.v[4] = Some(v);
        self
    }
    pub fn alpha(mut self, v: f32) -> Self {
        self.v[5] = Some(v);
        self
    }
    pub fn visible(mut self, v: bool) -> Self {
        self.visible = Some(v);
        self
    }
    /// `self` with `o`'s properties on top.
    pub fn merge(mut self, o: &P) -> Self {
        for i in 0..N {
            if o.v[i].is_some() {
                self.v[i] = o.v[i];
            }
        }
        if o.visible.is_some() {
            self.visible = o.visible;
        }
        self
    }
    /// Apply to full properties.
    pub fn apply(&self, mut p: Props) -> Props {
        for i in 0..N {
            if let Some(v) = self.v[i] {
                props_set(&mut p, i, v);
            }
        }
        if let Some(b) = self.visible {
            p.visible = b;
        }
        p
    }
}

#[derive(Clone, Debug)]
struct Tween {
    start: [f32; N],
    target: P,
    t0: f64,
    dur: f32,
    ease: Ease,
}

impl Tween {
    fn k(&self, now: f64) -> f32 {
        let t = if self.dur <= 0.0 { 1.0 } else { ((now - self.t0) / self.dur as f64).clamp(0.0, 1.0) as f32 };
        if t >= 1.0 { 1.0 } else { self.ease.at(t) }
    }
}

#[derive(Clone, Debug, Default)]
struct Clip {
    /// Script-set values on top of the authored ones.
    set: P,
    tween: Option<Tween>,
    /// Timeline: label it was sent to with `gotoAndPlay` and when.
    play: Option<(String, f64)>,
}

impl Clip {
    fn eval(&self, authored: Props, now: f64) -> Props {
        let mut p = self.set.apply(authored);
        if let Some(tw) = &self.tween {
            let k = tw.k(now);
            for i in 0..N {
                if let Some(e) = tw.target.v[i] {
                    props_set(&mut p, i, tw.start[i] + k * (e - tw.start[i]));
                }
            }
            if let Some(b) = tw.target.visible {
                p.visible = b;
            }
        }
        p
    }

    /// `tweenEnd(false)`: the tween's current values become script-set values.
    fn freeze(&mut self, authored: Props, now: f64) {
        let v = self.eval(authored, now);
        if let Some(tw) = self.tween.take() {
            for i in 0..N {
                if tw.target.v[i].is_some() {
                    self.set.v[i] = Some(props_get(&v, i));
                }
            }
            if tw.target.visible.is_some() {
                self.set.visible = tw.target.visible;
            }
        }
    }
}

/// What a queued script step does.
#[derive(Clone, Debug)]
pub enum Act {
    /// Assign properties.
    Set(P),
    /// `tweenEnd(false)`: stop the clip's tween where it is.
    End,
    /// `tweenEnd(true)`: jump the clip's tween to its end.
    Finish,
    /// `tweenTo(seconds, props, ease)`.
    To(P, f32, Ease),
    /// `tweenFrom(seconds, props, ease)`: jump to `props`, tween back to the current values.
    From(P, f32, Ease),
    /// `gotoAndPlay(label)`.
    Play(String),
}

/// Clip values of one screen, keyed by the screen's own clip names.
#[derive(Default)]
pub struct Anim {
    clips: HashMap<String, Clip>,
    queue: Vec<(f64, String, Act)>,
    /// Authored values of the clips (tween starts of never-set properties).
    authored: HashMap<String, Props>,
    rng: u64,
}

impl Anim {
    pub fn new(seed: u64) -> Self {
        Self { rng: seed | 1, ..Default::default() }
    }

    /// Forget every clip and queued step (the screen was closed and rebuilt from scratch).
    pub fn clear(&mut self) {
        self.clips.clear();
        self.queue.clear();
    }

    /// Drop queued steps of clips whose key starts with `prefix` and freeze their tweens (a
    /// script's `ClearIntervals` + `tweenEnd(false)`).
    pub fn cancel(&mut self, prefix: &str, now: f64) {
        self.queue.retain(|q| !q.1.starts_with(prefix));
        let authored = &self.authored;
        for (k, c) in self.clips.iter_mut() {
            if k.starts_with(prefix) {
                c.freeze(authored.get(k).copied().unwrap_or_default(), now);
            }
        }
    }

    /// Drop only the queued steps of clips whose key starts with `prefix` (`clearInterval`).
    pub fn cancel_queued(&mut self, prefix: &str) {
        self.queue.retain(|q| !q.1.starts_with(prefix));
    }

    /// Forget clips (values, tweens, queued steps and authored values) whose key starts with
    /// `prefix`: clips a script removed and attaches again (`removeMovieClip`, `attachMovie`).
    pub fn forget(&mut self, prefix: &str) {
        self.queue.retain(|q| !q.1.starts_with(prefix));
        self.clips.retain(|k, _| !k.starts_with(prefix));
        self.authored.retain(|k, _| !k.starts_with(prefix));
    }

    /// Set a clip's authored properties, replacing any registered before.
    pub fn reauthor(&mut self, key: &str, p: Props) {
        self.authored.insert(key.to_string(), p);
    }

    /// Remember a clip's authored properties (call before queueing steps on it).
    pub fn author(&mut self, key: &str, p: Props) {
        self.authored.entry(key.to_string()).or_insert(p);
    }

    /// The authored properties registered for `key`.
    pub fn authored(&self, key: &str) -> Props {
        self.authored.get(key).copied().unwrap_or_default()
    }

    /// Queue `act` on `key` at time `at` (real seconds).
    pub fn at(&mut self, at: f64, key: &str, act: Act) {
        self.queue.push((at, key.to_string(), act));
    }

    /// Run `act` now.
    pub fn now(&mut self, now: f64, key: &str, act: Act) {
        self.run(now, key, act);
    }

    fn run(&mut self, now: f64, key: &str, act: Act) {
        let authored = self.authored.get(key).copied().unwrap_or_default();
        let c = self.clips.entry(key.to_string()).or_default();
        match act {
            Act::Set(p) => c.set = c.set.merge(&p),
            Act::End => c.freeze(authored, now),
            Act::Finish => {
                if let Some(tw) = c.tween.take() {
                    c.set = c.set.merge(&tw.target);
                }
            }
            Act::To(target, dur, ease) => {
                c.freeze(authored, now);
                let base = c.set.apply(authored);
                let mut start = [0.0; N];
                for (i, s) in start.iter_mut().enumerate() {
                    *s = props_get(&base, i);
                }
                c.tween = Some(Tween { start, target, t0: now, dur, ease });
            }
            Act::From(from, dur, ease) => {
                // `tweenEnd(true)` then swap: the clip jumps to `from` and tweens back.
                if let Some(tw) = c.tween.take() {
                    c.set = c.set.merge(&tw.target);
                }
                let cur = c.set.apply(authored);
                let mut start = [0.0; N];
                let mut target = P::default();
                for (i, s) in start.iter_mut().enumerate() {
                    *s = from.v[i].unwrap_or(props_get(&cur, i));
                    if from.v[i].is_some() {
                        target.v[i] = Some(props_get(&cur, i));
                    }
                }
                if from.visible.is_some() {
                    c.set.visible = from.visible;
                }
                c.tween = Some(Tween { start, target, t0: now, dur, ease });
            }
            Act::Play(label) => c.play = Some((label, now)),
        }
    }

    /// Run the queued steps that are due, in time order.
    pub fn advance(&mut self, now: f64) {
        if self.queue.is_empty() {
            return;
        }
        self.queue.sort_by(|a, b| a.0.total_cmp(&b.0));
        let n = self.queue.iter().take_while(|q| q.0 <= now).count();
        let due: Vec<(f64, String, Act)> = self.queue.drain(..n).collect();
        for (t, k, a) in due {
            self.run(t, &k, a);
        }
    }

    /// The clip's current properties (from its registered authored ones).
    pub fn get(&self, key: &str, now: f64) -> Props {
        let a = self.authored(key);
        match self.clips.get(key) {
            Some(c) => c.eval(a, now),
            None => a,
        }
    }

    /// The clip's current properties if script ever touched it (an override for drawing).
    pub fn value(&self, key: &str, now: f64) -> Option<Props> {
        self.clips.get(key).filter(|c| c.tween.is_some() || c.set != P::default()).map(|c| c.eval(self.authored(key), now))
    }

    /// Overrides for drawing a movie clip whose clips are keyed `prefix/<path>`.
    pub fn ovr<'a>(&'a self, prefix: &'a str, now: f64) -> impl Fn(&str) -> Option<Props> + 'a {
        move |path: &str| self.value(&format!("{prefix}/{path}"), now)
    }

    /// The label the clip's timeline was last sent to and the seconds since.
    pub fn timeline(&self, key: &str, now: f64) -> Option<(&str, f32)> {
        self.clips.get(key).and_then(|c| c.play.as_ref()).map(|(l, t)| (l.as_str(), (now - t).max(0.0) as f32))
    }

    /// Something is still moving: a tween running, a step queued, or a timeline started in
    /// the last `timeline_secs`.
    pub fn busy(&self, now: f64, timeline_secs: f32) -> bool {
        !self.queue.is_empty()
            || self.clips.values().any(|c| c.tween.as_ref().is_some_and(|t| now < t.t0 + t.dur as f64) || c.play.as_ref().is_some_and(|(_, t)| now - t < timeline_secs as f64))
    }

    /// `Math.floor(Math.random() * (max - min + 1)) + min` (the classes' `randRange`), from a
    /// seeded generator so runs are repeatable.
    pub fn rand_range(&mut self, min: f32, max: f32) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        let r = (self.rng >> 11) as f64 / (1u64 << 53) as f64;
        (r * (max - min + 1.0) as f64).floor() as f32 + min
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eases_end_points() {
        for e in [Ease::None, Ease::StrongIn, Ease::StrongOut, Ease::StrongInOut, Ease::BackIn, Ease::BackOut, Ease::BackInOut] {
            assert!(e.at(0.0).abs() < 1e-5, "{e:?}");
            assert!((e.at(1.0) - 1.0).abs() < 1e-5, "{e:?}");
        }
        // Strong.easeOut at half time: 1 - 0.5^5.
        assert!((Ease::StrongOut.at(0.5) - 0.96875).abs() < 1e-5);
        // Back.easeOut overshoots.
        assert!(Ease::BackOut.at(0.7) > 1.0);
    }

    #[test]
    fn tween_from_set_value_to_target() {
        let mut a = Anim::new(1);
        a.author("c", Props::default());
        a.now(0.0, "c", Act::Set(P::new().alpha(0.0).x(100.0)));
        a.now(0.0, "c", Act::To(P::new().alpha(100.0).x(0.0), 1.0, Ease::None));
        let p = a.get("c", 0.5);
        assert!((p.alpha - 50.0).abs() < 1e-3 && (p.x - 50.0).abs() < 1e-3);
        let p = a.get("c", 2.0);
        assert!((p.alpha - 100.0).abs() < 1e-3 && p.x.abs() < 1e-3);
    }

    #[test]
    fn queued_steps_run_in_order() {
        let mut a = Anim::new(1);
        a.author("c", Props::default());
        a.at(0.2, "c", Act::To(P::new().scale(200.0), 0.1, Ease::None));
        a.at(0.0, "c", Act::Set(P::new().scale(50.0)));
        a.advance(0.1);
        assert!((a.get("c", 0.1).xscale - 50.0).abs() < 1e-3);
        a.advance(0.25);
        assert!((a.get("c", 0.25).xscale - 125.0).abs() < 1e-2);
        assert!(!a.busy(1.0, 0.0));
    }

    #[test]
    fn retarget_starts_from_current_value() {
        let mut a = Anim::new(1);
        a.author("c", Props::default());
        a.now(0.0, "c", Act::To(P::new().x(100.0), 1.0, Ease::None));
        a.now(0.5, "c", Act::To(P::new().x(0.0), 1.0, Ease::None));
        assert!((a.get("c", 0.5).x - 50.0).abs() < 1e-3);
        assert!((a.get("c", 1.0).x - 25.0).abs() < 1e-3);
    }

    #[test]
    fn assignment_during_tween_is_overwritten() {
        let mut a = Anim::new(1);
        a.author("c", Props::default());
        a.now(0.0, "c", Act::Set(P::new().alpha(0.0)));
        a.now(0.0, "c", Act::To(P::new().alpha(100.0), 1.0, Ease::None));
        a.now(0.0, "c", Act::Set(P::new().alpha(100.0)));
        assert!((a.get("c", 0.25).alpha - 25.0).abs() < 1e-3);
    }
}
