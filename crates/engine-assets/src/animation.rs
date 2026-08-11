//! Animation clips, sampling, and the player that turns time into a [`Pose`].
//!
//! ```text
//! AnimationClip ── tracks ──► sample(time) ──► Pose ──► Skeleton::skinning_matrices
//!       ▲                          ▲
//!       │                          │
//! AnimationPlayer holds: which clip, where in it, how fast, and what it is fading from.
//! ```
//!
//! # The smallest thing that can grow
//!
//! One clip playing, one cross-fade out of the clip before it. No state machine, no blend
//! tree, no layers, no additive poses, no inverse kinematics. The phase brief asks for the
//! minimum architecture that can grow, and every one of those features is a *consumer* of
//! [`AnimationPlayer::pose`] rather than a change to it: a blend tree is several players and
//! a [`Pose::blend_towards`], and a layer is a second player masked to a bone subset.
//!
//! # Why the player owns its buffers
//!
//! Sampling runs every frame for every visible character. A `Pose` is a `Vec<Transform>`, so
//! a sampler that returned one by value would allocate per character per frame. The player
//! keeps three poses — output, fade source, blend scratch — allocated once when it is built
//! from a skeleton, and every frame writes through them. Nothing in this module allocates
//! after construction.

use engine_core::math::{Quat, Vec3};
use engine_core::transform::Transform;

use crate::error::AssetError;
use crate::skeleton::{Pose, Skeleton};

/// One keyframe.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Key<T> {
    /// Seconds from the start of the clip.
    pub time: f32,
    /// The value at that moment.
    pub value: T,
}

impl<T> Key<T> {
    /// Builds a key.
    pub const fn new(time: f32, value: T) -> Self {
        Self { time, value }
    }
}

/// Interpolates a channel of keys at `time`.
///
/// Keys are assumed sorted, which [`BoneTrack::sorted`] guarantees. Before the first key and
/// after the last, the channel **holds** rather than extrapolating: a clip that ends at 0.8 s
/// and is sampled at 0.81 s should be at its last key, not a tenth of a second past a pose
/// nobody authored.
fn sample_channel<T: Copy>(keys: &[Key<T>], time: f32, mix: impl Fn(T, T, f32) -> T) -> Option<T> {
    match keys {
        [] => None,
        [only] => Some(only.value),
        _ => {
            // The index of the first key strictly after `time`.
            let next = keys.partition_point(|key| key.time <= time);
            if next == 0 {
                return Some(keys[0].value);
            }
            if next == keys.len() {
                return Some(keys[keys.len() - 1].value);
            }
            let before = &keys[next - 1];
            let after = &keys[next];
            let span = after.time - before.time;
            // Two keys at the same time is a step, not a division by zero.
            let t = if span > 1e-9 {
                (time - before.time) / span
            } else {
                0.0
            };
            Some(mix(before.value, after.value, t))
        }
    }
}

/// What one bone does over the length of a clip.
///
/// Three independent channels rather than one channel of `Transform`, because that is how
/// every authoring tool and glTF store it, and because a clip that only rotates — which is
/// most of them — then carries no translation or scale data at all.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BoneTrack {
    /// Which bone, as an index into [`Skeleton::bones`].
    pub bone: u16,
    /// Translation keys, in the parent's space.
    pub translation: Vec<Key<Vec3>>,
    /// Rotation keys, in the parent's space.
    pub rotation: Vec<Key<Quat>>,
    /// Scale keys.
    pub scale: Vec<Key<Vec3>>,
}

impl BoneTrack {
    /// An empty track for `bone`.
    #[must_use]
    pub fn new(bone: u16) -> Self {
        Self {
            bone,
            ..Self::default()
        }
    }

    /// The same track with rotation keys.
    #[must_use]
    pub fn with_rotation(mut self, keys: Vec<Key<Quat>>) -> Self {
        self.rotation = keys;
        self.sorted()
    }

    /// The same track with translation keys.
    #[must_use]
    pub fn with_translation(mut self, keys: Vec<Key<Vec3>>) -> Self {
        self.translation = keys;
        self.sorted()
    }

    /// The same track with scale keys.
    #[must_use]
    pub fn with_scale(mut self, keys: Vec<Key<Vec3>>) -> Self {
        self.scale = keys;
        self.sorted()
    }

    /// Sorts every channel by time.
    ///
    /// Sampling binary-searches, and a binary search over unsorted keys does not fail — it
    /// silently returns the wrong key, which shows up as one limb jittering.
    #[must_use]
    pub fn sorted(mut self) -> Self {
        self.translation.sort_by(|a, b| a.time.total_cmp(&b.time));
        self.rotation.sort_by(|a, b| a.time.total_cmp(&b.time));
        self.scale.sort_by(|a, b| a.time.total_cmp(&b.time));
        self
    }

    /// The last keyed moment in any channel.
    #[must_use]
    pub fn end_time(&self) -> f32 {
        let last = |times: &[f32]| times.last().copied().unwrap_or(0.0);
        let translation: Vec<f32> = self.translation.iter().map(|key| key.time).collect();
        let rotation: Vec<f32> = self.rotation.iter().map(|key| key.time).collect();
        let scale: Vec<f32> = self.scale.iter().map(|key| key.time).collect();
        last(&translation).max(last(&rotation)).max(last(&scale))
    }

    /// Whether this track keys anything at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.translation.is_empty() && self.rotation.is_empty() && self.scale.is_empty()
    }

    /// Writes this track's value at `time` over `transform`.
    ///
    /// Channels with no keys leave their component of the transform untouched, which is what
    /// makes a rotation-only track compose with a rest pose instead of flattening it.
    fn apply(&self, time: f32, transform: &mut Transform) {
        if let Some(translation) = sample_channel(&self.translation, time, Vec3::lerp) {
            transform.translation = translation;
        }
        if let Some(rotation) = sample_channel(&self.rotation, time, Quat::nlerp) {
            transform.rotation = rotation;
        }
        if let Some(scale) = sample_channel(&self.scale, time, Vec3::lerp) {
            transform.scale = scale;
        }
    }
}

/// A named animation: how long it is, whether it repeats, and what each bone does.
#[derive(Debug, Clone, PartialEq)]
pub struct AnimationClip {
    /// What the artist called it: `"idle"`, `"run"`, `"cast"`.
    pub name: String,
    /// Seconds from the first key to the last.
    pub duration: f32,
    /// Whether playback wraps at the end or holds the final pose.
    pub looping: bool,
    /// One entry per animated bone. Bones with no track keep their rest transform.
    pub tracks: Vec<BoneTrack>,
}

impl AnimationClip {
    /// Builds a clip, taking its duration from the last key in any track.
    ///
    /// A caller that wants a clip longer than its keys — an idle with a pause at the end —
    /// sets [`AnimationClip::duration`] afterwards.
    #[must_use]
    pub fn new(name: impl Into<String>, looping: bool, tracks: Vec<BoneTrack>) -> Self {
        let duration = tracks
            .iter()
            .map(BoneTrack::end_time)
            .fold(0.0_f32, f32::max);
        Self {
            name: name.into(),
            duration,
            looping,
            tracks,
        }
    }

    /// Wraps or clamps `time` into the clip, according to whether it loops.
    #[must_use]
    pub fn wrap(&self, time: f32) -> f32 {
        if self.duration <= 1e-6 {
            return 0.0;
        }
        if self.looping {
            time.rem_euclid(self.duration)
        } else {
            time.clamp(0.0, self.duration)
        }
    }

    /// Writes the clip's pose at `time` into `pose`, over whatever is already there.
    ///
    /// The caller seeds `pose` with the rest pose first. That is not an optimization, it is
    /// the semantics: a clip that animates six bones of a thirty-bone skeleton says nothing
    /// about the other twenty-four, and they must stay where the skeleton put them.
    ///
    /// # Errors
    ///
    /// Returns [`AssetError::Invalid`] if a track names a bone the pose does not have —
    /// which means the clip was bound to the wrong skeleton.
    pub fn sample(&self, time: f32, pose: &mut Pose) -> Result<(), AssetError> {
        let time = self.wrap(time);
        for track in &self.tracks {
            let Some(transform) = pose.locals_mut().get_mut(track.bone as usize) else {
                return Err(AssetError::invalid(
                    &self.name,
                    format!("track for bone {}", track.bone),
                    format!("the pose has only {} bones", pose.len()),
                ));
            };
            track.apply(time, transform);
        }
        Ok(())
    }
}

/// Whether a clip repeats.
///
/// A separate type from [`AnimationClip::looping`] so a caller can override an authored
/// clip's default — a cast animation is authored as a one-shot but a channelled spell wants
/// the same clip held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Playback {
    /// Repeat from the start.
    Looping,
    /// Play through and hold the final pose.
    Once,
}

/// Plays one clip at a time, cross-fading out of the previous one.
#[derive(Debug, Clone)]
pub struct AnimationPlayer {
    /// The rest pose, kept so every sample starts from a known state.
    rest: Pose,
    /// The pose produced by the last [`AnimationPlayer::update`].
    pose: Pose,
    /// The pose the character was in when the current clip started.
    fade_from: Pose,
    /// Scratch for the cross-fade, so blending allocates nothing.
    scratch: Pose,
    current: Option<usize>,
    time: f32,
    speed: f32,
    fade_remaining: f32,
    fade_duration: f32,
    finished: bool,
}

impl AnimationPlayer {
    /// A player resting in `skeleton`'s rest pose, playing nothing.
    #[must_use]
    pub fn new(skeleton: &Skeleton) -> Self {
        let rest = skeleton.rest_pose();
        Self {
            pose: rest.clone(),
            fade_from: rest.clone(),
            scratch: rest.clone(),
            rest,
            current: None,
            time: 0.0,
            speed: 1.0,
            fade_remaining: 0.0,
            fade_duration: 0.0,
            finished: false,
        }
    }

    /// Which clip is playing, as an index into the model's clips.
    #[must_use]
    pub fn current(&self) -> Option<usize> {
        self.current
    }

    /// Seconds into the current clip.
    #[must_use]
    pub fn time(&self) -> f32 {
        self.time
    }

    /// The playback rate. 1 is authored speed.
    #[must_use]
    pub fn speed(&self) -> f32 {
        self.speed
    }

    /// Sets the playback rate.
    ///
    /// Negative rates are clamped to zero. Playing a clip backwards is a feature nothing
    /// asks for, and the wrap arithmetic for it is a separate decision.
    pub fn set_speed(&mut self, speed: f32) {
        self.speed = speed.max(0.0);
    }

    /// Whether a non-looping clip has reached its end.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// Whether a cross-fade is in progress.
    #[must_use]
    pub fn is_blending(&self) -> bool {
        self.fade_remaining > 0.0
    }

    /// The pose to skin with.
    #[must_use]
    pub fn pose(&self) -> &Pose {
        &self.pose
    }

    /// Starts `clip`, cross-fading out of the current pose over `fade` seconds.
    ///
    /// **Asking for the clip that is already playing does nothing.** That is what makes this
    /// safe to call unconditionally from a state mapper every frame: the caller says what the
    /// character *should* be doing, not what changed, and the player decides whether that is
    /// news. Without it, a walk animation would restart from frame zero sixty times a second
    /// and the character would appear frozen mid-stride.
    pub fn play(&mut self, clip: usize, fade: f32) {
        if self.current == Some(clip) {
            return;
        }
        self.fade_from.copy_from(&self.pose);
        self.fade_duration = fade.max(0.0);
        self.fade_remaining = self.fade_duration;
        self.current = Some(clip);
        self.time = 0.0;
        self.finished = false;
    }

    /// Restarts the current clip from its beginning without a fade.
    ///
    /// What a repeated cast of the same spell needs: [`AnimationPlayer::play`] would
    /// correctly decide nothing changed, but the second cast really does want the animation
    /// from the top.
    pub fn restart(&mut self) {
        self.time = 0.0;
        self.finished = false;
    }

    /// Advances by `dt` seconds and rebuilds [`AnimationPlayer::pose`].
    ///
    /// # Errors
    ///
    /// Returns [`AssetError::Invalid`] if the clip is bound to a different skeleton than the
    /// one this player was built from.
    pub fn update(&mut self, dt: f32, clips: &[AnimationClip]) -> Result<(), AssetError> {
        let dt = dt.max(0.0);
        let Some(index) = self.current else {
            self.pose.copy_from(&self.rest);
            return Ok(());
        };
        let Some(clip) = clips.get(index) else {
            return Err(AssetError::invalid(
                "animation",
                format!("clip {index}"),
                format!("the model has {} clips", clips.len()),
            ));
        };

        self.time += dt * self.speed;
        if !clip.looping && self.time >= clip.duration {
            self.time = clip.duration;
            self.finished = true;
        }

        self.pose.copy_from(&self.rest);
        clip.sample(self.time, &mut self.pose)?;

        if self.fade_remaining > 0.0 {
            self.fade_remaining = (self.fade_remaining - dt).max(0.0);
            // How far *into* the new clip's pose the blend has travelled.
            let t = if self.fade_duration > 1e-6 {
                1.0 - self.fade_remaining / self.fade_duration
            } else {
                1.0
            };
            self.scratch.copy_from(&self.fade_from);
            self.scratch.blend_towards(&self.pose, t);
            std::mem::swap(&mut self.scratch, &mut self.pose);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skeleton::Bone;
    use engine_core::math::Mat4;

    fn two_bones() -> Skeleton {
        let bones = vec![
            Bone {
                name: "root".to_owned(),
                parent: None,
                rest: Transform::IDENTITY,
                inverse_bind: Mat4::IDENTITY,
            },
            Bone {
                name: "arm".to_owned(),
                parent: Some(0),
                rest: Transform::from_translation(Vec3::new(0.0, 1.0, 0.0)),
                inverse_bind: Mat4::IDENTITY,
            },
        ];
        match Skeleton::new(bones) {
            Ok(skeleton) => skeleton,
            Err(error) => panic!("the test skeleton must build: {error}"),
        }
    }

    fn slide_clip(looping: bool) -> AnimationClip {
        AnimationClip::new(
            "slide",
            looping,
            vec![BoneTrack::new(1).with_translation(vec![
                Key::new(0.0, Vec3::ZERO),
                Key::new(1.0, Vec3::new(10.0, 0.0, 0.0)),
            ])],
        )
    }

    #[test]
    fn a_clip_takes_its_duration_from_its_last_key() {
        assert_eq!(slide_clip(true).duration, 1.0);
    }

    #[test]
    fn sampling_between_keys_interpolates() {
        let clip = slide_clip(true);
        let mut pose = two_bones().rest_pose();
        assert!(clip.sample(0.25, &mut pose).is_ok());
        assert!((pose.locals()[1].translation.x - 2.5).abs() < 1e-5);
    }

    #[test]
    fn a_bone_with_no_track_keeps_its_rest_transform() {
        // The reason `sample` writes over a rest-seeded pose rather than building one.
        let skeleton = two_bones();
        let mut pose = skeleton.rest_pose();
        assert!(slide_clip(true).sample(0.5, &mut pose).is_ok());
        assert_eq!(pose.locals()[0], Transform::IDENTITY);
        // The arm's rest translation was replaced by the track, but its rotation was not
        // keyed and so survives.
        assert_eq!(pose.locals()[1].rotation, Quat::IDENTITY);
    }

    #[test]
    fn a_channel_holds_rather_than_extrapolating_past_its_last_key() {
        let clip = slide_clip(false);
        let mut pose = two_bones().rest_pose();
        assert!(clip.sample(50.0, &mut pose).is_ok());
        assert_eq!(pose.locals()[1].translation.x, 10.0);
    }

    #[test]
    fn a_looping_clip_wraps_and_a_one_shot_clamps() {
        assert!((slide_clip(true).wrap(2.25) - 0.25).abs() < 1e-6);
        assert_eq!(slide_clip(false).wrap(2.25), 1.0);
    }

    #[test]
    fn a_track_naming_a_bone_the_skeleton_lacks_is_reported() {
        let clip = AnimationClip::new("bad", true, vec![BoneTrack::new(9)]);
        let mut pose = two_bones().rest_pose();
        assert!(clip.sample(0.0, &mut pose).is_err());
    }

    #[test]
    fn playing_the_clip_that_is_already_playing_does_not_restart_it() {
        // Without this the state mapper, which calls `play` every frame, would freeze the
        // character on frame zero of its walk.
        let skeleton = two_bones();
        let clips = [slide_clip(true)];
        let mut player = AnimationPlayer::new(&skeleton);
        player.play(0, 0.0);
        assert!(player.update(0.5, &clips).is_ok());
        player.play(0, 0.2);
        assert_eq!(player.time(), 0.5, "the clip restarted");
        assert!(!player.is_blending());
    }

    #[test]
    fn switching_clips_cross_fades_out_of_the_pose_the_character_was_in() {
        let skeleton = two_bones();
        // The first clip is a one-shot, so a full second leaves it holding x = 10 rather
        // than wrapping back to where it started.
        let clips = [
            slide_clip(false),
            AnimationClip::new(
                "still",
                true,
                vec![BoneTrack::new(1).with_translation(vec![Key::new(0.0, Vec3::ZERO)])],
            ),
        ];
        let mut player = AnimationPlayer::new(&skeleton);
        player.play(0, 0.0);
        assert!(player.update(1.0, &clips).is_ok());
        assert!((player.pose().locals()[1].translation.x - 10.0).abs() < 1e-4);

        // Halfway through a 0.4 s fade into a clip that holds x = 0, x should be about 5.
        player.play(1, 0.4);
        assert!(player.update(0.2, &clips).is_ok());
        let x = player.pose().locals()[1].translation.x;
        assert!((x - 5.0).abs() < 0.5, "mid-fade x was {x}");
        assert!(player.is_blending());

        assert!(player.update(0.3, &clips).is_ok());
        assert!(!player.is_blending());
        assert!(player.pose().locals()[1].translation.x.abs() < 1e-4);
    }

    #[test]
    fn a_one_shot_clip_reports_when_it_is_done_and_a_looping_one_never_does() {
        let skeleton = two_bones();
        let mut player = AnimationPlayer::new(&skeleton);

        let once = [slide_clip(false)];
        player.play(0, 0.0);
        assert!(player.update(0.5, &once).is_ok());
        assert!(!player.is_finished());
        assert!(player.update(1.0, &once).is_ok());
        assert!(player.is_finished());

        let mut looping_player = AnimationPlayer::new(&skeleton);
        let forever = [slide_clip(true)];
        looping_player.play(0, 0.0);
        assert!(looping_player.update(9.0, &forever).is_ok());
        assert!(!looping_player.is_finished());
    }

    #[test]
    fn speed_scales_time_and_cannot_run_backwards() {
        let skeleton = two_bones();
        let clips = [slide_clip(true)];
        let mut player = AnimationPlayer::new(&skeleton);
        player.set_speed(2.0);
        player.play(0, 0.0);
        assert!(player.update(0.25, &clips).is_ok());
        assert_eq!(player.time(), 0.5);

        player.set_speed(-3.0);
        assert_eq!(player.speed(), 0.0);
    }

    #[test]
    fn a_player_with_no_clip_holds_the_rest_pose() {
        let skeleton = two_bones();
        let mut player = AnimationPlayer::new(&skeleton);
        assert!(player.update(1.0, &[]).is_ok());
        assert_eq!(player.pose(), &skeleton.rest_pose());
    }

    #[test]
    fn a_clip_index_the_model_does_not_have_is_reported() {
        let skeleton = two_bones();
        let mut player = AnimationPlayer::new(&skeleton);
        player.play(4, 0.0);
        assert!(player.update(0.1, &[]).is_err());
    }

    #[test]
    fn restarting_replays_the_same_clip_from_the_top() {
        let skeleton = two_bones();
        let clips = [slide_clip(false)];
        let mut player = AnimationPlayer::new(&skeleton);
        player.play(0, 0.0);
        assert!(player.update(1.5, &clips).is_ok());
        assert!(player.is_finished());
        player.restart();
        assert_eq!(player.time(), 0.0);
        assert!(!player.is_finished());
    }
}
