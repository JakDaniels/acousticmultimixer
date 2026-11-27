# Changelog

What changed in each release.

## Unreleased

- Every value of a room preset can be overridden while people talk (`reflection-delays`, `reflection-levels`,
  `wall-cutoff`, `reverb-decay`, `reverb-treble-decay`, `reverb-size`, `room-falloff`, `reflection-spread`,
  `reflection-diffusion`, `near-floor`), and `room-parameters` shows the values in effect; changing the room no
  longer cuts the reflections and reverb already sounding.
- A room can be described instead of tuned: its size (`room-width`, `room-depth`, `room-height`), what its walls,
  floor and ceiling are made of (`wall-material` …, or numbers in `wall-absorption` …) and what is in it
  (`room-contents`) give its reflections, cutoff and reverb (overrides still win); outdoors with a size is a room.

## 0.1.0 (2026-11-19)

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
- Rooms (`room`: `outdoor`, the default, `tiny-room`, `small-room`, `big-room`, `hall`, `long-hall`, `cave`,
  `cathedral`): each voice gets the early reflections of that kind of space, the later ones heard more from all
  around and relatively louder the further away it is.
- Late reverb in every room but `outdoor`: each voice gets a decaying tail (its own delay network, so nobody hears
  their own), falling with distance like the reflections; `reverb-level` turns it up, down or off. Reflections
  now fall with distance faster than before (three quarters of the direct sound's rate in dB, not half), and
  closer than `near-distance` (2 m) the room sound is turned down, so a voice right next to you stands out.
- Voices fade out smoothly over the last quarter of the range (`taper`, 0.75) instead of cutting off at
  `max-range`.
- Fixed: mixing slowed down many times over (stuttering) once voices fell to exact silence after speaking, as the
  filters decayed into denormal numbers; they are now flushed to zero while mixing.
