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
