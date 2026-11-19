# Changelog

What changed in each release.

## Unreleased

- First version: a mix-minus mixer (every output hears all the other inputs), based on gst-plugins-rs's
  `minus1mixer`, with a fixed number of participants (`max-participants`) so that joining and leaving never
  renegotiates.
- Stereo outputs, with each voice's level from its distance to the listener and its pan from the direction
  (SpatialAudioServer's distance models and pan), set through pad properties (position, orientation, a separate
  listening pose, gain, mute, per-listener trims); changes ramp over one buffer, so moving does not click.
- Direction by a head model (`direction-model=head`, the default): the far ear hears a voice up to 0.66 ms later,
  quieter and duller; `direction-model=pan` keeps the stereo pan. Distant voices lose high frequencies
  (`air-absorption`), and voices behind the listener are duller in both ears (`rear-shadow`), so front and back
  sound different.
- Rooms (`room`: `outdoor`, the default, `small-room`, `hall`, `cave`): each voice gets the early reflections of
  that kind of space, the later ones heard more from all around and relatively louder the further away it is.
- Late reverb in every room but `outdoor`: each voice gets a decaying tail (its own delay network, so nobody hears
  their own), falling with distance like the reflections; `reverb-level` turns it up, down or off. Reflections
  now fall with distance faster than before (three quarters of the direct sound's rate in dB, not half).
- Voices fade out smoothly over the last quarter of the range (`taper`, 0.75) instead of cutting off at
  `max-range`.
- Fixed: mixing slowed down many times over (stuttering) once voices fell to exact silence after speaking, as the
  filters decayed into denormal numbers; they are now flushed to zero while mixing.
