// Copyright 2026 Simon Annetts/Jak Daniels
// SPDX-License-Identifier: MPL-2.0

//! GStreamer plugin `acousticmultimixer`: a mix-minus audio mixer that places every voice in a shared virtual
//! acoustic space, giving each listener their own stereo mix. See PLAN.md.
//!
//! The structure (a bin holding an AudioAggregator-based mixer and a splitter) comes from gst-plugins-rs's
//! `audiomultimixer` plugin by Tim-Philipp Müller.

#![allow(unused_doc_comments)]

use gst::glib;
use gst::prelude::*;

mod acousticmultimixer;
mod dsp;
mod mixer;
mod pad;
mod rooms;
mod scene;
mod spatial;
mod splitter;

glib::wrapper! {
    pub struct MultiMixerElement(ObjectSubclass<mixer::MultiMixerElement>) @extends gst_audio::AudioAggregator, gst_base::Aggregator, gst::Element, gst::Object;
}

glib::wrapper! {
    pub struct AcousticMultiMixer(ObjectSubclass<acousticmultimixer::AcousticMultiMixer>) @extends gst::Bin, gst::Element, gst::Object, @implements gst::ChildProxy;
}

glib::wrapper! {
    pub struct AcousticMultiMixerPad(ObjectSubclass<pad::SinkPad>) @extends gst::GhostPad, gst::ProxyPad, gst::Pad, gst::Object;
}

glib::wrapper! {
    pub struct MultiMixerSplitter(ObjectSubclass<splitter::Splitter>) @extends gst::Element, gst::Object;
}

fn plugin_init(plugin: &gst::Plugin) -> Result<(), glib::BoolError> {
    // The mixer and the splitter stay internal to the bin; the pad type and the enums are part
    // of its interface.
    AcousticMultiMixerPad::static_type().mark_as_plugin_api(gst::PluginAPIFlags::empty());
    spatial::DistanceModel::static_type().mark_as_plugin_api(gst::PluginAPIFlags::empty());
    spatial::DirectionModel::static_type().mark_as_plugin_api(gst::PluginAPIFlags::empty());
    rooms::Room::static_type().mark_as_plugin_api(gst::PluginAPIFlags::empty());

    gst::Element::register(
        Some(plugin),
        "acousticmultimixer",
        gst::Rank::NONE,
        AcousticMultiMixer::static_type(),
    )?;

    Ok(())
}

gst::plugin_define!(
    acousticmultimixer,
    env!("CARGO_PKG_DESCRIPTION"),
    plugin_init,
    concat!(env!("CARGO_PKG_VERSION"), "-", env!("COMMIT_ID")),
    "MPL-2.0",
    env!("CARGO_PKG_NAME"),
    env!("CARGO_PKG_NAME"),
    env!("CARGO_PKG_REPOSITORY"),
    env!("BUILD_REL_DATE")
);

#[cfg(test)]
mod tests;
