// Copyright 2026 Simon Annetts/Jak Daniels
// SPDX-License-Identifier: MPL-2.0

//! GStreamer plugin `acousticmultimixer`: a mix-minus audio mixer that places every voice in a shared virtual
//! acoustic space, giving each listener their own stereo mix. See PLAN.md.

use gst::glib;

fn plugin_init(_plugin: &gst::Plugin) -> Result<(), glib::BoolError> {
    // The element is registered here from step 1 (PLAN.md).
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
