# Changelog

What changed in each release.

## Unreleased

- First version: a mix-minus mixer (every output hears all the other inputs), based on gst-plugins-rs's
  `minus1mixer`, with a fixed number of participants (`max-participants`) so that joining and leaving never
  renegotiates.
- Stereo outputs, with each voice's level from its distance to the listener and its pan from the direction
  (SpatialAudioServer's distance models and pan), set through pad properties (position, orientation, a separate
  listening pose, gain, mute, per-listener trims); changes ramp over one buffer, so moving does not click.
